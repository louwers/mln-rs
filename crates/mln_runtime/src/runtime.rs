//! A runtime: the thread that its maps live on, and the file sources, cache
//! and offline regions that they share.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;

use mln_engine::Registry;
#[cfg(not(target_family = "wasm"))]
use mln_engine::offline::RegionDatabase;
use mln_engine::storage::http_header_transform::{HttpHeaderTransform, HttpHeaders};
use mln_engine::storage::network_status::NetworkStatus;
use mln_engine::storage::resource::Kind;
use mln_engine::storage::resource_transform::{FinishedCallback, ResourceTransform};
use mln_engine::storage::{FileSource, Loader, OnlineFileSource, ResourceOptions};
use mln_engine::text::font_face_engine::FontFaceEngine;
use mln_style::error::Error;
use mln_style::time::Instant;
use mln_text::GlyphCache;
use mln_util::pool::Pool;
use mln_util::queue::{Queue, TimedTask};
use mln_util::thread::Joined;
use parking_lot::Mutex;

use crate::completion::{Completion, Resolver};
use crate::event::{EventHandler, RuntimeEvent};
use crate::log_target;
use crate::map::{Applied, Map, MapEntry, MapId, MapOptions, MapShared, Published};
use crate::offline::Offline;
use crate::resources::{CacheDatabaseOperation, ProviderFileSource, ResourceHooks, ResourceProvider};

/// Where a runtime keeps its assets and its cache, and the network status
/// its requests follow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// The directory that `asset://` URLs load from. `None` uses the working
    /// directory, or on Android the app's assets, which
    /// `mln_engine::storage::android::init` gives the library.
    pub asset_path: Option<PathBuf>,
    /// The cache database file, which holds the ambient cache and the offline
    /// regions. Its directory must be writable. `None` keeps the cache in
    /// memory.
    pub cache_path: Option<PathBuf>,
    /// The network status that the runtime's network requests follow. `None`
    /// gives the runtime one of its own, which stays online.
    pub network_status: Option<Arc<NetworkStatus>>,
    /// In the browser, where the browser's fetch looks for a response that
    /// the host fetched ahead before it fetches a whole resource:
    /// `take(url)` returns a promise of the `Response` fetched for exactly
    /// `url`, at most once for each, or undefined. A promise that rejects
    /// is fetched again. `None` fetches every resource.
    #[cfg(target_family = "wasm")]
    pub fetch_preloads: Option<js_sys::Function>,
}

/// A runtime: a thread of its own that holds maps, and the resource loading,
/// cache and offline regions that they share.
///
/// Every call returns at once. Commands run on the runtime's thread in the
/// order they were sent, across the runtime and all its maps, and report
/// through a [`Completion`]. `Clone` is another handle to the same runtime,
/// and any thread may use one.
///
/// The runtime lives until [`release`](Self::release) or
/// [`dispose`](Self::dispose), or until its last handle and its last map
/// are dropped. A handler or provider that holds a handle keeps it alive
/// until then.
///
/// Unless the runtime was released or disposed, the drop of the last of its
/// handles and maps returns once the runtime's threads have ended: its own
/// and its file sources'. A worker that loads over HTTP and is stuck in a
/// name lookup or a TCP connect is waited for up to a second, then left to
/// end alone. The host's handlers, providers and transforms that the runtime
/// holds are dropped on its thread meanwhile, so they mustn't wait for the
/// thread that drops it. A drop on a thread of the library, such as in a
/// handler or callback that the library runs, doesn't wait: it leaves the
/// runtime's threads to end alone.
///
/// A panic in the work of the runtime or its maps is a bug in the library,
/// and stops the runtime instead of the process. It logs the panic, the
/// command that panicked fails with
/// [`ErrorKind::Native`](mln_style::error::ErrorKind), those that waited are
/// cancelled, and the commands sent to the runtime and its maps after fail
/// with `ErrorKind::Native`. Release its maps and the runtime then.
#[derive(Clone)]
pub struct Runtime {
    pub(crate) shared: Arc<RuntimeShared>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").field("thread", &self.shared.thread).finish()
    }
}

struct Lifecycle {
    released: bool,
    live_maps: usize,
    shut_down: bool,
}

pub(crate) struct RuntimeShared {
    queue: CoreQueue,
    /// The tasks that [`post`](Self::post) sent and the thread hasn't run.
    queued: Arc<AtomicUsize>,
    thread: ThreadId,
    lifecycle: Mutex<Lifecycle>,
    /// Why the runtime stopped, once a panic in its work stopped it.
    failure: Arc<OnceLock<Error>>,
    next_map_id: AtomicU64,
    /// The glyphs that the renderers of the runtime's maps parse, which they
    /// share.
    pub(crate) glyph_cache: Arc<GlyphCache>,
    /// What the renderers of the runtime's maps load font faces with: in
    /// the browser, the shaper module, which loads the first time a style
    /// declares font faces.
    pub(crate) font_face_engine: Arc<dyn FontFaceEngine>,
    pub(crate) registry: Arc<Registry>,
    /// Where the runtime's background work runs: its maps', and their
    /// render sessions'.
    pub(crate) pool: Pool,
    /// The runtime's thread, which the drop waits for after the shutdown
    /// unless a release or disposal detached it, except in the browser,
    /// where the thread is the host's.
    thread_end: Option<Joined>,
}

impl RuntimeShared {
    /// Runs `task` on the runtime's thread, after everything sent before it,
    /// in the burst of the tasks queued with it.
    pub(crate) fn post(&self, task: impl FnOnce(&mut RuntimeCore, &mut Outbox) + Send + 'static) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        self.queue.post(move |core| run(core, task, true));
    }

    /// Runs `task` with `value` on the runtime's thread, after everything
    /// sent before it, or `stopped` with `value` there instead once the
    /// runtime has let go of its state, for work that ends the same either
    /// way, such as a release.
    pub(crate) fn post_or_stopped<V: Send + 'static>(
        &self,
        value: V,
        task: impl FnOnce(&mut RuntimeCore, &mut Outbox, V) + Send + 'static,
        stopped: impl FnOnce(V) + Send + 'static,
    ) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        self.queue.post(move |core| {
            if core.is_some() {
                run(core, move |core, outbox| task(core, outbox, value), true);
            } else {
                stopped(value);
            }
        });
    }

    /// Runs `task` on the runtime's thread, after everything sent before it,
    /// outside the bursts of the tasks that [`post`](Self::post) sends: it
    /// neither starts one nor counts in one.
    pub(crate) fn post_apart(&self, task: impl FnOnce(&mut RuntimeCore, &mut Outbox) + Send + 'static) {
        post(&self.queue, task);
    }

    /// Runs `task` with `value` on the runtime's thread, after everything
    /// sent before it, or `instead` with `value` on this thread when the
    /// runtime has shut down, so that what must happen either way isn't lost
    /// with the runtime's queue.
    pub(crate) fn post_or<V: Send + 'static>(
        &self,
        value: V,
        task: impl FnOnce(&mut RuntimeCore, &mut Outbox, V) + Send + 'static,
        instead: impl FnOnce(V),
    ) {
        // Posting with the lock held orders the task before a shutdown.
        let lifecycle = self.lifecycle.lock();
        if lifecycle.shut_down {
            drop(lifecycle);
            instead(value);
        } else {
            self.post(move |core, outbox| task(core, outbox, value));
        }
    }

    pub(crate) fn thread(&self) -> ThreadId {
        self.thread
    }

    /// A completion that the runtime's thread resolves.
    pub(crate) fn completion<T: Send + 'static>(&self) -> (Completion<T>, Resolver<T>) {
        Completion::new(Some(self.thread))
    }

    /// Leaves the runtime's thread to end alone, for a release or disposal,
    /// which don't wait for it.
    fn detach_thread(&self) {
        if let Some(thread) = &self.thread_end {
            thread.detach();
        }
    }

    fn check_live(&self) -> Result<(), Error> {
        if self.lifecycle.lock().released {
            return Err(released());
        }
        self.check_running()
    }

    /// Fails once a panic in the runtime's work stopped it.
    pub(crate) fn check_running(&self) -> Result<(), Error> {
        self.failure.get().map_or(Ok(()), |failure| Err(failure.clone()))
    }

    /// A map of the runtime was released or dropped. A disposed runtime shuts
    /// down with its last map.
    pub(crate) fn map_released(&self) {
        let shut_down = {
            let mut lifecycle = self.lifecycle.lock();
            lifecycle.live_maps -= 1;
            let shut_down = lifecycle.released && lifecycle.live_maps == 0 && !lifecycle.shut_down;
            lifecycle.shut_down |= shut_down;
            shut_down
        };
        if shut_down {
            self.shut_down(None);
        }
    }

    /// Tears the runtime down on its thread, after the commands sent before.
    fn shut_down(&self, done: Option<Resolver<()>>) {
        let queue = self.queue.clone();
        self.queue.post(move |core| {
            // Dropping the database waits for its work, whose results queue
            // up behind this task, so they resolve before `done`.
            drop(core.take());
            if let Some(done) = done {
                queue.post(move |_| done.resolve(Ok(())));
            }
            queue.stop();
        });
    }
}

impl Drop for RuntimeShared {
    /// Shuts the runtime down unless it has, then waits for its thread as
    /// `thread_end` drops.
    fn drop(&mut self) {
        if !self.lifecycle.get_mut().shut_down {
            self.shut_down(None);
        }
    }
}

#[cfg(target_family = "wasm")]
fn no_database() -> Error {
    Error::unsupported("the browser build has no cache database: the browser caches the requests")
}

#[cfg(not(target_family = "wasm"))]
fn font_face_engine() -> Arc<dyn FontFaceEngine> {
    mln_engine::text::font_face_engine::default_engine()
}

#[cfg(target_family = "wasm")]
fn font_face_engine() -> Arc<dyn FontFaceEngine> {
    Arc::new(mln_web_host::shaper::ShaperModuleEngine)
}

/// The most worker threads of a runtime's pool.
#[cfg(not(target_family = "wasm"))]
const POOL_THREADS: usize = 4;

/// The pool of a new runtime.
#[cfg(not(target_family = "wasm"))]
fn runtime_pool() -> Pool {
    Pool::new(POOL_THREADS)
}

/// The pool of a new runtime: in the browser, one without threads, which
/// the instance's pump runs.
#[cfg(target_family = "wasm")]
fn runtime_pool() -> Pool {
    Pool::local()
}

fn released() -> Error {
    Error::invalid_state("the runtime was released")
}

/// The work of a runtime's thread, which owns the runtime's state: `None`
/// once the runtime shut down.
pub(crate) type CoreQueue = Queue<Option<RuntimeCore>>;

/// Where a runtime's background work runs, and where its results come back.
#[derive(Clone)]
pub(crate) struct Background {
    pub(crate) pool: Pool,
    pub(crate) queue: CoreQueue,
}

/// Posts `task` to the runtime thread that runs `queue`.
pub(crate) fn post(queue: &CoreQueue, task: impl FnOnce(&mut RuntimeCore, &mut Outbox) + Send + 'static) {
    queue.post(move |core| run(core, task, false));
}

/// Posts `task` to the runtime thread that runs `queue`, to run once
/// `deadline` passed.
pub(crate) fn post_at(
    queue: &CoreQueue,
    deadline: Instant,
    task: impl FnOnce(&mut RuntimeCore, &mut Outbox) + Send + 'static,
) -> TimedTask {
    queue.post_at(deadline, move |core| run(core, task, false))
}

/// Runs `task` on the runtime's state, then delivers what it sends to the
/// host. A task that comes after the runtime shut down drops, which cancels
/// its completions. A task that [`RuntimeShared::post`] sent runs in its
/// burst.
fn run(core: &mut Option<RuntimeCore>, task: impl FnOnce(&mut RuntimeCore, &mut Outbox), in_burst: bool) {
    let Some(state) = core.as_mut() else { return };
    let mut outbox = Outbox { deliveries: std::mem::take(&mut state.spare_deliveries) };
    let ran = mln_util::panic::catch(|| {
        if in_burst {
            state.begin_burst_task();
        }
        task(state, &mut outbox);
        if in_burst {
            state.end_burst_task(&mut outbox);
        }
    });
    match ran {
        Ok(()) => state.spare_deliveries = outbox.deliver(),
        Err(message) => {
            fail(core, &message);
            outbox.deliver();
        }
    }
}

/// Stops the runtime after a panic in its work, a bug in the library: its
/// state drops, as at a shutdown, which cancels the completions of the
/// commands that it held, and the commands sent from now on fail.
fn fail(core: &mut Option<RuntimeCore>, message: &str) {
    let Some(state) = core.take() else { return };
    let error = Error::native(format!("the runtime stopped after a panic in the library: {message}"));
    log::error!(target: log_target::CRASH, "{}", error.message);
    let _ = state.failure.set(error);
    // The panic may have left the state half-changed, so its drop may panic
    // too.
    let _ = mln_util::panic::catch(move || drop(state));
}

/// What a task sends to the host: event batches and results, in order. They
/// go out once the task is done with the runtime's state, so host code that
/// runs never finds it borrowed.
#[derive(Default)]
pub(crate) struct Outbox {
    deliveries: Vec<Box<dyn FnOnce()>>,
}

impl Outbox {
    pub(crate) fn push(&mut self, delivery: impl FnOnce() + 'static) {
        self.deliveries.push(Box::new(delivery));
    }

    pub(crate) fn resolve<T: 'static>(&mut self, resolver: Resolver<T>, result: Result<T, Error>) {
        // A success whose result is discarded runs nothing.
        if resolver.discards() && result.is_ok() {
            return resolver.resolve(result);
        }
        self.push(move || resolver.resolve(result));
    }

    /// Delivers what the outbox holds, and returns its emptied buffer.
    pub(crate) fn deliver(mut self) -> Vec<Box<dyn FnOnce()>> {
        for delivery in self.deliveries.drain(..) {
            delivery();
        }
        self.deliveries
    }
}

/// The runtime's state, on its thread.
pub(crate) struct RuntimeCore {
    /// Declared first, so the maps go before the file sources they use.
    pub(crate) maps: BTreeMap<MapId, MapEntry>,
    pub(crate) event_handler: Option<EventHandler>,
    hooks: Arc<ResourceHooks>,
    resource_options: ResourceOptions,
    /// The runtime's file sources, made at their first use and held for the
    /// runtime's life, so that each map, and each transform, finds the same
    /// ones. A runtime that never loads a resource starts none of their
    /// threads.
    loader: OnceCell<Arc<dyn FileSource>>,
    #[cfg(not(target_family = "wasm"))]
    database: OnceCell<Arc<RegionDatabase>>,
    /// The library's own network loading, behind the host's provider.
    online: OnceCell<Arc<OnlineFileSource>>,
    /// What the library's own network loading follows and, in the browser,
    /// looks in first.
    network_options: NetworkOptions,
    network: OnceCell<Arc<dyn FileSource>>,
    /// The runtime's thread's work.
    pub(crate) queue: CoreQueue,
    pub(crate) registry: Arc<Registry>,
    /// [`RuntimeShared::pool`].
    pub(crate) pool: Pool,
    /// [`RuntimeShared::queued`].
    queued: Arc<AtomicUsize>,
    /// [`RuntimeShared::failure`].
    failure: Arc<OnceLock<Error>>,
    /// The tasks of the burst in progress that haven't finished.
    burst_left: usize,
    /// Whether the maps hold their updates back for the burst.
    holding: bool,
    /// The buffer of the outbox that the next task takes, so that each task
    /// reuses the last one's.
    spare_deliveries: Vec<Box<dyn FnOnce()>>,
}

impl Drop for RuntimeCore {
    /// Stops the file sources' threads, and waits for them, after the maps
    /// have gone. The sources can outlive the runtime's handles of them: a
    /// response's callback holds the loader while it runs on a source's
    /// thread, and a provider's request holds the online source, so their
    /// last handles can drop where a drop doesn't wait, such as on the
    /// source's own thread.
    fn drop(&mut self) {
        self.maps.clear();
        #[cfg(not(target_family = "wasm"))]
        if let Some(database) = self.database.get() {
            database.shut_down();
        }
        if let Some(online) = self.online.get() {
            online.shut_down();
        }
    }
}

/// What the library's own network loading of a runtime follows.
struct NetworkOptions {
    status: Arc<NetworkStatus>,
    /// [`RuntimeOptions::fetch_preloads`].
    #[cfg(target_family = "wasm")]
    fetch_preloads: Option<js_sys::Function>,
}

impl RuntimeCore {
    fn new(
        resource_options: ResourceOptions,
        network_options: NetworkOptions,
        queue: CoreQueue,
        queued: Arc<AtomicUsize>,
        failure: Arc<OnceLock<Error>>,
        registry: Arc<Registry>,
        pool: Pool,
    ) -> Self {
        RuntimeCore {
            maps: BTreeMap::new(),
            event_handler: None,
            hooks: Arc::new(ResourceHooks::default()),
            resource_options,
            loader: OnceCell::new(),
            #[cfg(not(target_family = "wasm"))]
            database: OnceCell::new(),
            online: OnceCell::new(),
            network_options,
            network: OnceCell::new(),
            queue,
            registry,
            pool,
            queued,
            failure,
            burst_left: 0,
            holding: false,
            spare_deliveries: Vec::new(),
        }
    }

    /// Starts a task that [`RuntimeShared::post`] sent. The tasks that were
    /// queued when a burst's first one starts make up the burst, and while a
    /// burst of more than one runs, the maps hold their updates back, so that
    /// a render session draws what the whole burst changed in one frame. A
    /// task sent during a burst is in the next one, so a host that keeps
    /// sending holds no update back for longer than a burst.
    fn begin_burst_task(&mut self) {
        let queued = self.queued.fetch_sub(1, Ordering::SeqCst);
        if self.burst_left == 0 {
            self.burst_left = queued;
            self.holding = queued > 1;
            if self.holding {
                self.maps.values_mut().for_each(|entry| entry.core.hold_updates());
            }
        }
    }

    /// Ends a task of the burst, and after its last, shows what the maps
    /// held back, and tells their sessions what ran meanwhile.
    fn end_burst_task(&mut self, outbox: &mut Outbox) {
        self.burst_left = self.burst_left.saturating_sub(1);
        if self.burst_left > 0 || !std::mem::take(&mut self.holding) {
            return;
        }
        let ids: Vec<MapId> = self.maps.keys().copied().collect();
        for id in ids {
            if let Some(entry) = self.maps.get_mut(&id) {
                entry.core.release_updates();
            }
            crate::map::flush(self, id, outbox);
            if let Some(entry) = self.maps.get_mut(&id) {
                crate::map::report_held(entry);
            }
        }
    }

    /// Whether the maps hold their updates back for a burst.
    pub(crate) fn holding_updates(&self) -> bool {
        self.holding
    }

    /// The loader that the runtime's maps share.
    pub(crate) fn loader(&self) -> &Arc<dyn FileSource> {
        self.loader.get_or_init(|| {
            let builder = Loader::builder(&self.pool)
                .asset_path(&self.resource_options.asset_path)
                .network(Arc::clone(self.network()));
            #[cfg(not(target_family = "wasm"))]
            let builder = builder.database(Arc::<RegionDatabase>::clone(self.database()));
            Arc::new(builder.build())
        })
    }

    /// The library's own network loading.
    fn online(&self) -> &Arc<OnlineFileSource> {
        self.online.get_or_init(|| {
            #[cfg(not(target_family = "wasm"))]
            let online = OnlineFileSource::new(Arc::clone(&self.network_options.status));
            #[cfg(target_family = "wasm")]
            let online = crate::web::online(
                Arc::clone(&self.network_options.status),
                self.network_options.fetch_preloads.clone(),
            );
            Arc::new(online)
        })
    }

    /// The runtime's network: the host's resource provider, in front of the
    /// library's own loading.
    fn network(&self) -> &Arc<dyn FileSource> {
        self.network
            .get_or_init(|| Arc::new(ProviderFileSource::new(Arc::clone(self.online()), Arc::clone(&self.hooks))))
    }

    /// Where the runtime's background work runs, and its results come back.
    pub(crate) fn background(&self) -> Background {
        Background { pool: self.pool.clone(), queue: self.queue.clone() }
    }

    /// Sends `events` to the event handler.
    pub(crate) fn send_events(&self, events: Vec<RuntimeEvent>, outbox: &mut Outbox) {
        if events.is_empty() {
            return;
        }
        if let Some(handler) = self.event_handler.clone() {
            outbox.push(move || handler(&events));
        }
    }

    /// The cache database, which every runtime but the browser build's has.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn database(&self) -> &Arc<RegionDatabase> {
        self.database.get_or_init(|| {
            Arc::new(mln_engine::offline::region_database(
                &self.resource_options.cache_path,
                Some(Arc::clone(self.network())),
                Arc::clone(self.registry.style()),
            ))
        })
    }
}

/// Delivers a database result on the runtime's thread, so it keeps its
/// place among the runtime's other results and events. It needs no runtime
/// state, so a result that arrives while the runtime shuts down, which waits
/// for the database's work, still resolves rather than being cancelled.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn reply_on<T: Send + 'static, E: Into<Error>>(
    queue: CoreQueue,
    resolver: Resolver<T>,
) -> impl FnOnce(Result<T, E>) + Send + 'static {
    move |result| {
        let result = result.map_err(Into::into);
        queue.post(move |_| resolver.resolve(result));
    }
}

/// The runtime's resource options, with the cache file created when there
/// is one, so that a path the runtime can't use fails here.
fn resource_options(options: &RuntimeOptions) -> Result<ResourceOptions, Error> {
    let mut resource_options = ResourceOptions::default();
    if let Some(asset_path) = &options.asset_path {
        resource_options.asset_path = utf8(asset_path, "asset path")?.to_owned();
    }
    #[cfg(target_os = "android")]
    if options.asset_path.is_none() {
        resource_options.asset_path = mln_engine::storage::android::APP_ASSETS.to_owned();
    }
    if options.cache_path.is_some() && cfg!(target_family = "wasm") {
        return Err(Error::unsupported("the browser build has no cache database: the browser caches the requests"));
    }
    if let Some(cache_path) = &options.cache_path {
        let path = utf8(cache_path, "cache path")?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(cache_path)
            .map_err(|error| Error::invalid_argument(format!("can't open the cache database {path}: {error}")))?;
        resource_options.cache_path = path.to_owned();
    }
    Ok(resource_options)
}

fn utf8<'a>(path: &'a Path, what: &str) -> Result<&'a str, Error> {
    path.to_str().ok_or_else(|| Error::invalid_argument(format!("the {what} {} isn't UTF-8", path.display())))
}

impl Runtime {
    /// Starts a runtime and its thread, with the built-in registrations.
    ///
    /// In the browser, the runtime's thread is the one that calls this, which
    /// a wasm instance has only one of, and the runtime never blocks it: its
    /// commands run in tasks of their own on the browser's event loop
    /// (`mln_web_host::pump`), never inside the call that sends them.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) when the
    /// cache database file can't be opened or created, and in the browser
    /// with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) for a cache path.
    pub fn new(options: &RuntimeOptions) -> Result<Runtime, Error> {
        Self::with_registry(options, Registry::with_builtins())
    }

    /// Starts a runtime and its thread, whose maps parse their styles
    /// against `registry`, and whose renderers and tile workers use it. In
    /// the browser, the tile workers' Web Workers must serve with registries
    /// filled by the same calls (`mln_web_host::tile_worker::serve`).
    ///
    /// Fails as [`new`](Self::new) does.
    pub fn with_registry(options: &RuntimeOptions, registry: Arc<Registry>) -> Result<Runtime, Error> {
        let resource_options = resource_options(options)?;
        let network_options = NetworkOptions {
            status: options.network_status.clone().unwrap_or_default(),
            #[cfg(target_family = "wasm")]
            fetch_preloads: options.fetch_preloads.clone(),
        };
        let queued = Arc::new(AtomicUsize::new(0));
        let failure = Arc::new(OnceLock::new());
        let queue = CoreQueue::default();
        let pool = runtime_pool();
        // A panic in a task of the runtime's maps or their sessions stops the
        // runtime, as one on its thread does.
        let failing = queue.downgrade();
        pool.set_panic_handler(Some(Arc::new(move |message| {
            if let Some(queue) = failing.upgrade() {
                queue.post(move |core| fail(core, &message));
            }
        })));
        let (thread, thread_end) = Self::start(
            resource_options,
            network_options,
            queue.clone(),
            Arc::clone(&queued),
            Arc::clone(&failure),
            Arc::clone(&registry),
            pool.clone(),
        )?;
        Ok(Runtime {
            shared: Arc::new(RuntimeShared {
                queue,
                queued,
                thread,
                lifecycle: Mutex::new(Lifecycle { released: false, live_maps: 0, shut_down: false }),
                failure,
                next_map_id: AtomicU64::new(1),
                glyph_cache: Arc::default(),
                font_face_engine: font_face_engine(),
                registry,
                pool,
                thread_end,
            }),
        })
    }

    /// Builds the runtime's state on a thread of its own, which owns it and
    /// runs `queue` until the runtime shuts down.
    #[cfg(not(target_family = "wasm"))]
    fn start(
        resource_options: ResourceOptions,
        network_options: NetworkOptions,
        queue: CoreQueue,
        queued: Arc<AtomicUsize>,
        failure: Arc<OnceLock<Error>>,
        registry: Arc<Registry>,
        pool: Pool,
    ) -> Result<(ThreadId, Option<Joined>), Error> {
        let thread = mln_util::thread::spawn("Runtime", move || {
            let own_pool = pool.clone();
            let mut core = Some(RuntimeCore::new(
                resource_options,
                network_options,
                queue.clone(),
                queued,
                failure,
                registry,
                pool,
            ));
            queue.run(&mut core);
            // What comes after the shutdown drops at once, so tasks that hold
            // the queue can't keep it alive.
            queue.close();
            drop(core);
            // Nothing of the runtime runs after this, but a render session
            // that kept its graphics forgot its renderer, which holds the
            // pool: its workers stop here rather than with the last handle.
            own_pool.shut_down();
        })
        .map_err(|error| Error::native(format!("can't start the runtime's thread: {error}")))?;
        let thread_id = thread.thread().id();
        // The thread ends by itself once the runtime shuts down.
        Ok((thread_id, Some(Joined::new(thread))))
    }

    /// Builds the runtime's state on this thread, where the instance's pump
    /// runs `queue` with it until the runtime shuts down.
    #[cfg(target_family = "wasm")]
    #[expect(clippy::unnecessary_wraps, reason = "the threads of other builds can fail to start")]
    fn start(
        resource_options: ResourceOptions,
        network_options: NetworkOptions,
        queue: CoreQueue,
        queued: Arc<AtomicUsize>,
        failure: Arc<OnceLock<Error>>,
        registry: Arc<Registry>,
        pool: Pool,
    ) -> Result<(ThreadId, Option<Joined>), Error> {
        let core = RuntimeCore::new(resource_options, network_options, queue.clone(), queued, failure, registry, pool);
        let pool = core.pool.clone();
        crate::web::drive(&pool, queue, Some(core));
        Ok((std::thread::current().id(), None))
    }

    /// What the runtime's maps parse against, their renderers draw and
    /// their tile workers lay out.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.shared.registry
    }

    /// Sends a command to the runtime's thread.
    fn command<T: Send + 'static>(
        &self,
        command: impl FnOnce(&mut RuntimeCore, &mut Outbox, Resolver<T>) + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.shared.check_live()?;
        let (completion, resolver) = self.shared.completion();
        self.shared.post(move |core, outbox| command(core, outbox, resolver));
        Ok(completion)
    }

    /// Sends a command that runs on the runtime's state and completes at once.
    fn apply<T: Send + 'static>(
        &self,
        apply: impl FnOnce(&mut RuntimeCore) -> Result<T, Error> + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.command(move |core, outbox, resolver| {
            let result = apply(core);
            outbox.resolve(resolver, result);
        })
    }

    /// Creates a map on the runtime.
    ///
    /// The map's handle is ready at once, and the commands sent to it wait
    /// for the map to be built. If building fails, the completion reports
    /// why and those commands fail with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for an
    /// empty size or a pixel ratio that isn't finite and positive.
    pub fn create_map(&self, options: MapOptions) -> Result<(Map, Completion<Applied>), Error> {
        let size = options.map.size;
        let pixel_ratio = options.map.pixel_ratio;
        if size.is_empty() {
            return Err(Error::invalid_argument("the map's size must not be empty"));
        }
        if !pixel_ratio.is_finite() || pixel_ratio <= 0.0 {
            return Err(Error::invalid_argument("the map's pixel ratio must be finite and positive"));
        }
        self.shared.check_running()?;
        {
            let mut lifecycle = self.shared.lifecycle.lock();
            if lifecycle.released {
                return Err(released());
            }
            lifecycle.live_maps += 1;
        }
        if options.system_fonts {
            mln_system_fonts::prefetch();
        }
        let id = MapId(self.shared.next_map_id.fetch_add(1, Ordering::Relaxed));
        let published = Arc::new(Published::new(options.event_mask));
        let map = Map::new(MapShared::new(id, Arc::clone(&self.shared), &options, Arc::clone(&published)));
        let (completion, resolver) = self.shared.completion();
        self.shared.post(move |core, outbox| crate::map::create(core, id, &options, published, resolver, outbox));
        Ok((map, completion))
    }

    /// Sends the events of the runtime and its maps to `handler`, in batches
    /// on the runtime's thread. It replaces the handler before.
    pub fn set_event_handler(
        &self,
        handler: impl Fn(&[RuntimeEvent]) + Send + Sync + 'static,
    ) -> Result<Completion<()>, Error> {
        let handler: EventHandler = Arc::new(handler);
        self.command(move |core, outbox, resolver| {
            let previous = core.event_handler.replace(handler);
            outbox.push(move || drop(previous));
            outbox.resolve(resolver, Ok(()));
        })
    }

    /// Stops sending events to a handler.
    pub fn clear_event_handler(&self) -> Result<Completion<()>, Error> {
        self.command(move |core, outbox, resolver| {
            let previous = core.event_handler.take();
            outbox.push(move || drop(previous));
            outbox.resolve(resolver, Ok(()));
        })
    }

    /// Serves the runtime's network requests that `provider` serves through
    /// it, and the rest as usual. It replaces the provider before.
    pub fn set_resource_provider(&self, provider: impl ResourceProvider) -> Result<Completion<()>, Error> {
        self.replace_provider(Some(Box::new(provider)))
    }

    /// Removes the resource provider, so the library loads every request
    /// itself.
    pub fn clear_resource_provider(&self) -> Result<Completion<()>, Error> {
        self.replace_provider(None)
    }

    fn replace_provider(&self, provider: Option<Box<dyn ResourceProvider>>) -> Result<Completion<()>, Error> {
        self.command(move |core, outbox, resolver| {
            let previous = core.hooks.set_provider(provider);
            outbox.push(move || drop(previous));
            outbox.resolve(resolver, Ok(()));
        })
    }

    /// Rewrites the URLs of the runtime's network requests before they go
    /// out: `transform` gets the resource's kind and URL, and answers through
    /// the callback, now or later and on any thread, with the URL to load or
    /// the error to fail the request with. It replaces the transform before.
    pub fn set_resource_transform(
        &self,
        transform: impl Fn(Kind, &str, FinishedCallback) + Send + Sync + 'static,
    ) -> Result<Completion<()>, Error> {
        let transform: ResourceTransform = Arc::new(transform);
        self.apply(move |core| {
            core.online().set_resource_transform(Some(transform));
            core.hooks.set_transform_set(true);
            Ok(())
        })
    }

    /// Stops rewriting request URLs.
    pub fn clear_resource_transform(&self) -> Result<Completion<()>, Error> {
        self.apply(move |core| {
            core.online().set_resource_transform(None);
            core.hooks.set_transform_set(false);
            Ok(())
        })
    }

    /// Adds the headers that `transform` returns to the runtime's HTTP
    /// requests: it gets the resource's kind and the URL after any resource
    /// transform, and runs for each attempt of a request, which waits for
    /// it. Names that the library manages, such as `Range` or `User-Agent`,
    /// are dropped, and a later value of a name replaces an earlier one. An
    /// `Accept` replaces the one the library sends for tiles and images. It
    /// replaces the transform before.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) in the
    /// browser, where the browser sends the requests.
    pub fn set_http_header_transform(
        &self,
        transform: impl Fn(Kind, &str) -> HttpHeaders + Send + Sync + 'static,
    ) -> Result<Completion<()>, Error> {
        if cfg!(target_family = "wasm") {
            return Err(Error::unsupported("the browser sends the requests and sets their headers"));
        }
        let transform = HttpHeaderTransform::new(Some(Arc::new(move |kind: Kind, url: &str| {
            crate::resources::sanitize_headers(transform(kind, url))
        })));
        self.apply(move |core| {
            core.online().set_http_header_transform(transform);
            Ok(())
        })
    }

    /// Stops adding headers to HTTP requests.
    pub fn clear_http_header_transform(&self) -> Result<Completion<()>, Error> {
        self.apply(move |core| {
            core.online().set_http_header_transform(HttpHeaderTransform::default());
            Ok(())
        })
    }

    /// Runs a maintenance action on the cache database.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) in the
    /// browser, which has no cache database.
    #[cfg(target_family = "wasm")]
    pub fn run_cache_database_operation(&self, operation: CacheDatabaseOperation) -> Result<Completion<()>, Error> {
        let _ = operation;
        Err(no_database())
    }

    /// Runs a maintenance action on the cache database.
    #[cfg(not(target_family = "wasm"))]
    pub fn run_cache_database_operation(&self, operation: CacheDatabaseOperation) -> Result<Completion<()>, Error> {
        self.command(move |core, _, resolver| {
            let database = core.database();
            let reply = reply_on(core.queue.clone(), resolver);
            match operation {
                CacheDatabaseOperation::Clear => database.clear_ambient_cache(reply),
                CacheDatabaseOperation::Invalidate => database.invalidate_ambient_cache(reply),
                CacheDatabaseOperation::Pack => database.pack_database(reply),
                CacheDatabaseOperation::Reset => database.reset_database(reply),
            }
        })
    }

    /// Limits the ambient cache to `size` bytes. Offline regions don't count
    /// toward it.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) in the
    /// browser, which has no cache database.
    #[cfg(target_family = "wasm")]
    pub fn set_maximum_ambient_cache_size(&self, size: u64) -> Result<Completion<()>, Error> {
        let _ = size;
        Err(no_database())
    }

    /// Limits the ambient cache to `size` bytes. Offline regions don't count
    /// toward it.
    #[cfg(not(target_family = "wasm"))]
    pub fn set_maximum_ambient_cache_size(&self, size: u64) -> Result<Completion<()>, Error> {
        self.command(move |core, _, resolver| {
            core.database().set_maximum_ambient_cache_size(size, reply_on(core.queue.clone(), resolver));
        })
    }

    /// The offline regions in the runtime's cache database.
    #[must_use]
    pub fn offline(&self) -> Offline<'_> {
        Offline::new(self)
    }

    /// Shuts the runtime down. The completion reports when its thread has
    /// let go of its maps' resources and its file sources, and nothing waits
    /// for the thread to end after that.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) while the
    /// runtime has maps that aren't released, or when the runtime was
    /// released already. Every call on the runtime fails the same way after.
    pub fn release(&self) -> Result<Completion<()>, Error> {
        {
            let mut lifecycle = self.shared.lifecycle.lock();
            if lifecycle.released {
                return Err(released());
            }
            if lifecycle.live_maps > 0 {
                return Err(Error::invalid_state("release the runtime's maps first"));
            }
            lifecycle.released = true;
            lifecycle.shut_down = true;
        }
        self.shared.detach_thread();
        let (completion, resolver) = self.shared.completion();
        self.shared.shut_down(Some(resolver));
        Ok(completion)
    }

    /// Releases the runtime whatever its state, without waiting: it shuts
    /// down once its last map is released, and nothing waits for its thread
    /// to end. Every call on the runtime fails with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) after.
    pub fn dispose(&self) -> Result<(), Error> {
        let shut_down = {
            let mut lifecycle = self.shared.lifecycle.lock();
            if lifecycle.released {
                return Err(released());
            }
            lifecycle.released = true;
            let shut_down = lifecycle.live_maps == 0;
            lifecycle.shut_down |= shut_down;
            shut_down
        };
        self.shared.detach_thread();
        if shut_down {
            self.shared.shut_down(None);
        }
        Ok(())
    }

    /// Sends a command for the offline API.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn offline_command<T: Send + 'static>(
        &self,
        command: impl FnOnce(&RegionDatabase, &RuntimeCore, Resolver<T>) + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.command(move |core, _, resolver| command(core.database(), core, resolver))
    }
}

// Handles cross threads; the core never leaves its own.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Runtime>();
    assert_send_sync::<Map>();
};
