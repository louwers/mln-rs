//! A runtime: the thread that its maps live on, and the file sources, cache
//! and offline regions that they share.

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::sync::mpsc;
use std::thread::ThreadId;

use mln_actor::{RunLoop, RunLoopHandle};
#[cfg(not(target_family = "wasm"))]
use mln_storage::DatabaseFileSource;
use mln_storage::http_header_transform::{HttpHeaderTransform, HttpHeaders};
use mln_storage::resource::Kind;
use mln_storage::resource_transform::{FinishedCallback, ResourceTransform};
use mln_storage::{FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_text::GlyphCache;
use mln_util::error::Error;
use mln_util::thread::Joined;
use parking_lot::Mutex;

use crate::completion::{Completion, Resolver};
use crate::event::{EventHandler, RuntimeEvent};
use crate::map::{Applied, Map, MapEntry, MapId, MapOptions, MapShared, Published};
use crate::offline::Offline;
use crate::resources::{CacheDatabaseOperation, ProviderFileSource, ResourceHooks, ResourceProvider};

/// Where a runtime keeps its assets and its cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// The directory that `asset://` URLs load from. `None` uses the working
    /// directory, or on Android the app's assets, which
    /// `mln_storage::android::init` gives the library.
    pub asset_path: Option<PathBuf>,
    /// The cache database file, which holds the ambient cache and the offline
    /// regions. Its directory must be writable. `None` keeps the cache in
    /// memory.
    pub cache_path: Option<PathBuf>,
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
    run_loop: RunLoopHandle,
    /// The tasks that [`post`](Self::post) sent and the thread hasn't run.
    queued: Arc<AtomicUsize>,
    thread: ThreadId,
    lifecycle: Mutex<Lifecycle>,
    next_map_id: AtomicU64,
    /// The glyphs that the renderers of the runtime's maps parse, which they
    /// share.
    pub(crate) glyph_cache: Arc<GlyphCache>,
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
        self.run_loop.invoke(move || run(task, true));
    }

    /// Runs `task` with `value` on the runtime's thread, after everything
    /// sent before it, or `instead` with `value` on this thread when the
    /// runtime has shut down, so that what must happen either way isn't lost
    /// with the runtime's loop.
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
        if self.lifecycle.lock().released { Err(released()) } else { Ok(()) }
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
        let run_loop = self.run_loop.clone();
        self.run_loop.invoke(move || {
            let core = CORE.with(|core| core.borrow_mut().take());
            // Dropping the database waits for its work, whose results queue
            // up behind this task, so they resolve before `done`.
            drop(core);
            if let Some(done) = done {
                run_loop.invoke(move || done.resolve(Ok(())));
            }
            run_loop.stop();
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

fn released() -> Error {
    Error::invalid_state("the runtime was released")
}

thread_local! {
    /// The state of the runtime whose thread this is.
    static CORE: RefCell<Option<RuntimeCore>> = const { RefCell::new(None) };
}

/// Posts `task` to the runtime thread whose loop is `run_loop`.
pub(crate) fn post(run_loop: &RunLoopHandle, task: impl FnOnce(&mut RuntimeCore, &mut Outbox) + Send + 'static) {
    run_loop.invoke(move || run(task, false));
}

/// Runs `task` on the runtime's state, then delivers what it sends to the
/// host, with the state unborrowed. A task that comes after the runtime shut
/// down drops, which cancels its completions. A task that
/// [`RuntimeShared::post`] sent runs in its burst.
fn run(task: impl FnOnce(&mut RuntimeCore, &mut Outbox), in_burst: bool) {
    let mut outbox = Outbox::default();
    let unrun = CORE.with(|core| match core.borrow_mut().as_mut() {
        Some(core) => {
            if in_burst {
                core.begin_burst_task();
            }
            task(core, &mut outbox);
            if in_burst {
                core.end_burst_task(&mut outbox);
            }
            None
        }
        None => Some(task),
    });
    drop(unrun);
    outbox.deliver();
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
        self.push(move || resolver.resolve(result));
    }

    pub(crate) fn deliver(self) {
        for delivery in self.deliveries {
            delivery();
        }
    }
}

/// The runtime's state, on its thread.
pub(crate) struct RuntimeCore {
    /// Declared first, so the maps go before the file sources they use.
    pub(crate) maps: BTreeMap<MapId, MapEntry>,
    pub(crate) event_handler: Option<EventHandler>,
    hooks: Arc<ResourceHooks>,
    file_sources: FileSourceManager,
    resource_options: ResourceOptions,
    /// The runtime's file sources, made at their first use and held for the
    /// runtime's life, so that each map, and each transform, finds the same
    /// ones. A runtime that never loads a resource starts none of their
    /// threads.
    loader: OnceCell<Option<Arc<dyn FileSource>>>,
    database: OnceCell<Option<Arc<dyn FileSource>>>,
    network: OnceCell<Option<Arc<dyn FileSource>>>,
    pub(crate) run_loop: RunLoopHandle,
    /// [`RuntimeShared::queued`].
    queued: Arc<AtomicUsize>,
    /// The tasks of the burst in progress that haven't finished.
    burst_left: usize,
    /// Whether the maps hold their updates back for the burst.
    holding: bool,
}

impl RuntimeCore {
    fn new(resource_options: ResourceOptions, run_loop: RunLoopHandle, queued: Arc<AtomicUsize>) -> Self {
        let hooks = Arc::new(ResourceHooks::default());
        let file_sources = FileSourceManager::new();
        let network_hooks = Arc::clone(&hooks);
        file_sources.register_file_source_factory(
            FileSourceType::Network,
            Arc::new(move |_: &FileSourceManager, options: &ResourceOptions| {
                Arc::new(ProviderFileSource::new(options, Arc::clone(&network_hooks)))
            }),
        );
        RuntimeCore {
            maps: BTreeMap::new(),
            event_handler: None,
            hooks,
            file_sources,
            resource_options,
            loader: OnceCell::new(),
            database: OnceCell::new(),
            network: OnceCell::new(),
            run_loop,
            queued,
            burst_left: 0,
            holding: false,
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
    /// held back.
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
        }
    }

    /// The manager that a map takes its file sources from, with the
    /// runtime's file sources made.
    pub(crate) fn file_sources(&self) -> (&FileSourceManager, &ResourceOptions) {
        self.file_source(&self.network, FileSourceType::Network);
        self.file_source(&self.database, FileSourceType::Database);
        self.file_source(&self.loader, FileSourceType::ResourceLoader);
        (&self.file_sources, &self.resource_options)
    }

    /// The runtime's file source of `type_` that `cell` holds, made at the
    /// first call.
    fn file_source<'a>(
        &self,
        cell: &'a OnceCell<Option<Arc<dyn FileSource>>>,
        type_: FileSourceType,
    ) -> Option<&'a Arc<dyn FileSource>> {
        cell.get_or_init(|| self.file_sources.get_file_source(type_, &self.resource_options)).as_ref()
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
    pub(crate) fn database(&self) -> Result<&DatabaseFileSource, Error> {
        self.file_source(&self.database, FileSourceType::Database)
            .and_then(|database| database.downcast_ref::<DatabaseFileSource>())
            .ok_or_else(|| Error::unsupported("the runtime has no cache database"))
    }

    fn network(&self) -> Result<&Arc<dyn FileSource>, Error> {
        self.file_source(&self.network, FileSourceType::Network)
            .ok_or_else(|| Error::unsupported("the runtime has no network file source"))
    }
}

/// Delivers a database result on the runtime's thread, so it keeps its
/// place among the runtime's other results and events. It needs no runtime
/// state, so a result that arrives while the runtime shuts down, which waits
/// for the database's work, still resolves rather than being cancelled.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn reply_on<T: Send + 'static, E: Into<Error>>(
    run_loop: RunLoopHandle,
    resolver: Resolver<T>,
) -> impl FnOnce(Result<T, E>) + Send + 'static {
    move |result| {
        let result = result.map_err(Into::into);
        run_loop.invoke(move || resolver.resolve(result));
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
        resource_options.asset_path = mln_storage::android::APP_ASSETS.to_owned();
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
    /// Starts a runtime and its thread.
    ///
    /// In the browser, the runtime's thread is the one that calls this, which
    /// a wasm instance has only one of, and the runtime never blocks it: its
    /// commands run in tasks of their own on the browser's event loop
    /// (`mln_web_host::pump`), never inside the call that sends them.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) when the
    /// cache database file can't be opened or created, and in the browser
    /// with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a cache path.
    pub fn new(options: &RuntimeOptions) -> Result<Runtime, Error> {
        let resource_options = resource_options(options)?;
        let queued = Arc::new(AtomicUsize::new(0));
        let (run_loop, thread, thread_end) = Self::start(resource_options, Arc::clone(&queued))?;
        Ok(Runtime {
            shared: Arc::new(RuntimeShared {
                run_loop,
                queued,
                thread,
                lifecycle: Mutex::new(Lifecycle { released: false, live_maps: 0, shut_down: false }),
                next_map_id: AtomicU64::new(1),
                glyph_cache: Arc::default(),
                thread_end,
            }),
        })
    }

    /// Builds the runtime's state on a thread of its own, which runs its loop
    /// until the runtime shuts down.
    #[cfg(not(target_family = "wasm"))]
    fn start(
        resource_options: ResourceOptions,
        queued: Arc<AtomicUsize>,
    ) -> Result<(RunLoopHandle, ThreadId, Option<Joined>), Error> {
        let (started, start) = mpsc::channel();
        let core_queued = queued;
        let thread = mln_util::thread::spawn("Runtime", move || {
            let run_loop = RunLoop::new();
            let core = RuntimeCore::new(resource_options, run_loop.handle(), core_queued);
            CORE.with(|cell| *cell.borrow_mut() = Some(core));
            let _ = started.send(run_loop.handle());
            run_loop.run();
            let core = CORE.with(|cell| cell.borrow_mut().take());
            drop(core);
        })
        .map_err(|error| Error::native(format!("can't start the runtime's thread: {error}")))?;
        let thread_id = thread.thread().id();
        // The thread ends by itself once the runtime shuts down.
        let thread = Joined::new(thread);
        let run_loop = start.recv().map_err(|_| Error::native("the runtime's thread stopped as it started"))?;
        Ok((run_loop, thread_id, Some(thread)))
    }

    /// Builds the runtime's state on this thread, under a loop of its own
    /// that the instance's pump runs until the runtime shuts down.
    #[cfg(target_family = "wasm")]
    fn start(
        resource_options: ResourceOptions,
        queued: Arc<AtomicUsize>,
    ) -> Result<(RunLoopHandle, ThreadId, Option<Joined>), Error> {
        crate::web::install();
        let handle = RunLoopHandle::new();
        let run_loop = RunLoop::adopt(&handle).ok_or_else(|| Error::native("the runtime's loop runs already"))?;
        let core = {
            let _current = run_loop.enter();
            RuntimeCore::new(resource_options, handle.clone(), queued)
        };
        let mut core = Some(core);
        crate::web::drive(run_loop, move |run| {
            // The core is the thread's while its loop runs.
            CORE.with(|cell| std::mem::swap(&mut *cell.borrow_mut(), &mut core));
            run();
            CORE.with(|cell| std::mem::swap(&mut *cell.borrow_mut(), &mut core));
        });
        Ok((handle, std::thread::current().id(), None))
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
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind).
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an
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
        {
            let mut lifecycle = self.shared.lifecycle.lock();
            if lifecycle.released {
                return Err(released());
            }
            lifecycle.live_maps += 1;
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
            core.network()?.set_resource_transform(Some(transform));
            core.hooks.set_transform_set(true);
            Ok(())
        })
    }

    /// Stops rewriting request URLs.
    pub fn clear_resource_transform(&self) -> Result<Completion<()>, Error> {
        self.apply(move |core| {
            core.network()?.set_resource_transform(None);
            core.hooks.set_transform_set(false);
            Ok(())
        })
    }

    /// Adds the headers that `transform` returns to the runtime's HTTP
    /// requests: it gets the resource's kind and the URL after any resource
    /// transform, and runs for each attempt of a request, which waits for
    /// it. Names that the library manages, such as `Range` or `User-Agent`,
    /// are dropped, and a later value of a name replaces an earlier one. It
    /// replaces the transform before.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) in the
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
            core.network()?.set_http_header_transform(transform);
            Ok(())
        })
    }

    /// Stops adding headers to HTTP requests.
    pub fn clear_http_header_transform(&self) -> Result<Completion<()>, Error> {
        self.apply(move |core| {
            core.network()?.set_http_header_transform(HttpHeaderTransform::default());
            Ok(())
        })
    }

    /// Runs a maintenance action on the cache database.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) in the
    /// browser, which has no cache database.
    #[cfg(target_family = "wasm")]
    pub fn run_cache_database_operation(&self, operation: CacheDatabaseOperation) -> Result<Completion<()>, Error> {
        let _ = operation;
        Err(no_database())
    }

    /// Runs a maintenance action on the cache database.
    #[cfg(not(target_family = "wasm"))]
    pub fn run_cache_database_operation(&self, operation: CacheDatabaseOperation) -> Result<Completion<()>, Error> {
        self.command(move |core, outbox, resolver| {
            let database = match core.database() {
                Ok(database) => database,
                Err(error) => return outbox.resolve(resolver, Err(error)),
            };
            let reply = reply_on(core.run_loop.clone(), resolver);
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
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) in the
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
        self.command(move |core, outbox, resolver| match core.database() {
            Ok(database) => database.set_maximum_ambient_cache_size(size, reply_on(core.run_loop.clone(), resolver)),
            Err(error) => outbox.resolve(resolver, Err(error)),
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
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) while the
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
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) after.
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
        command: impl FnOnce(&DatabaseFileSource, &RuntimeCore, Resolver<T>) + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.command(move |core, outbox, resolver| match core.database() {
            Ok(database) => command(database, core, resolver),
            Err(error) => outbox.resolve(resolver, Err(error)),
        })
    }
}

// Handles cross threads; the core never leaves its own.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Runtime>();
    assert_send_sync::<Map>();
};
