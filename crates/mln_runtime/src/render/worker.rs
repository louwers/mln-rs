//! A render session's thread: its renderer, its frame demands, and the work
//! that the handle sends it.

// The browser build renders into no borrowed ring.
#![cfg_attr(target_family = "wasm", allow(unreachable_code, unused_variables))]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use mln_actor::RunLoopHandle;
use mln_gfx::{BackendScope, ConsumerSync, Handoff, RendererBackend, ScopeType, SyncKind};
use mln_map::{MapView, RenderMode};
use mln_renderer::{Renderer, RendererEvent, RendererEvents, Update, UpdateReader};
use mln_style::collection::ImmutableVector;
use mln_style::source_impl;
use mln_style::sources::{GeoJSONData, geojson_source_impl};
use mln_text::GlyphCache;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::time::Instant;

use super::frame::{DemandMode, FrameDemand, FrameDisposition, FrameResult};
use super::local::{LocalTarget, Pending, PendingReads};
use super::session::{Published, RenderSessionState, SessionShared, WorkId, physical_size};
use super::target::{BorrowedRing, Device, DeviceLease, Driver, ExportKind, ExportedRing, Surface};
use crate::completion::Resolver;
use crate::map::Published as MapPublished;
use crate::runtime::Outbox;

/// Where a thread keeps the session whose work it runs.
pub(crate) type Slot = Rc<RefCell<Option<Worker>>>;

thread_local! {
    /// The session whose work this thread runs now.
    static CURRENT: RefCell<Option<Slot>> = const { RefCell::new(None) };
}

/// Makes a session the one whose work this thread runs, until it drops.
pub(crate) struct Entered {
    previous: Option<Slot>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = self.previous.take());
    }
}

/// Makes the session in `slot` the one whose work this thread runs, until
/// the guard drops: a host thread runs several sessions' work in turn.
pub(crate) fn enter(slot: &Slot) -> Entered {
    Entered { previous: CURRENT.with(|current| current.borrow_mut().replace(Rc::clone(slot))) }
}

/// Runs `task` on the session whose loop is `run_loop`, after the work sent
/// before.
pub(crate) fn post(run_loop: &RunLoopHandle, task: impl FnOnce(&mut Worker, &mut Outbox) + Send + 'static) {
    run_loop.invoke(move || run(task));
}

/// Runs `task` on the session, then delivers what it sends to the host, with
/// the session unborrowed and unlocked: a driver call, which the delivery is
/// part of. A session that was abandoned lets go of its graphics objects
/// first.
fn run(task: impl FnOnce(&mut Worker, &mut Outbox)) {
    let Some(slot) = CURRENT.with(|current| current.borrow().clone()) else { return };
    let Some(shared) = slot.borrow().as_ref().map(|worker| Arc::clone(&worker.shared)) else { return };
    let _call = shared.enter_call();
    let mut outbox = Outbox::default();
    if let Some(worker) = slot.borrow_mut().as_mut() {
        let _gate = shared.gate.lock();
        if !worker.torn_down && shared.state.lock().lifecycle == RenderSessionState::Abandoned {
            worker.tear_down(&mut outbox);
        }
        task(worker, &mut outbox);
    }
    shared.wait_for_senders();
    outbox.deliver();
}

/// Work for the session's thread.
pub(crate) type Task = Box<dyn FnOnce(&mut Worker, &mut Outbox) + Send>;

/// Tells the session's thread that a resize sent to the map has landed:
/// [`land`](Self::land) when the resize reaches the session, or on drop
/// when it never does.
pub(crate) struct ResizeLanding(Option<RunLoopHandle>);

impl ResizeLanding {
    pub(crate) fn new(run_loop: RunLoopHandle) -> Self {
        ResizeLanding(Some(run_loop))
    }

    pub(crate) fn land(mut self, worker: &mut Worker, outbox: &mut Outbox) {
        self.0 = None;
        worker.resize_landed(outbox);
    }
}

impl Drop for ResizeLanding {
    fn drop(&mut self) {
        if let Some(run_loop) = self.0.take() {
            post(&run_loop, Worker::resize_landed);
        }
    }
}

/// What a session's thread needs to build its renderer.
pub(crate) struct Init {
    pub(crate) target: TargetInit,
    pub(crate) size: Size,
    pub(crate) scale_factor: f64,
    pub(crate) glyph_cache: Arc<GlyphCache>,
    /// Whether the renderer keeps tiles that left the view.
    pub(crate) tile_cache_enabled: bool,
    /// Why the session's device couldn't be made before the build: in the
    /// browser, whose WebGPU makes devices asynchronously.
    pub(crate) failure: Option<Error>,
}

impl TargetInit {
    /// In the browser, whether the target renders with WebGPU without the
    /// host's device: the session asks the browser for one before it builds.
    #[cfg(target_family = "wasm")]
    fn needs_web_gpu_device(&self) -> bool {
        match self {
            TargetInit::Offscreen(None) => cfg!(feature = "webgpu"),
            TargetInit::Surface { device: None, surface } => surface.presents_with_web_gpu(),
            _ => false,
        }
    }

    #[cfg(all(target_family = "wasm", feature = "webgpu"))]
    fn set_device(&mut self, made: Device) {
        match self {
            TargetInit::Offscreen(device @ None) | TargetInit::Surface { device: device @ None, .. } => {
                *device = Some(made);
            }
            _ => {}
        }
    }
}

/// In the browser, makes the device that the session's build needs, if
/// any: the browser's WebGPU makes one asynchronously. The future resolves
/// once it is in the session's [`Init`], or its failure is.
#[cfg(target_family = "wasm")]
pub(crate) fn prepare(shared: &Arc<SessionShared>) -> impl Future<Output = ()> + 'static {
    let needs_device = shared
        .state
        .lock()
        .init
        .as_ref()
        .and_then(|init| init.downcast_ref::<Init>())
        .is_some_and(|init| init.target.needs_web_gpu_device());
    let shared = Arc::clone(shared);
    async move {
        if !needs_device {
            return;
        }
        #[cfg(feature = "webgpu")]
        let made = mln_webgpu::Device::request().await.map(Device::WebGpu);
        #[cfg(not(feature = "webgpu"))]
        let made: Result<Device, Error> = Err(Error::unsupported("this build has no WebGPU"));
        let mut state = shared.state.lock();
        if let Some(init) = state.init.as_mut().and_then(|init| init.downcast_mut::<Init>()) {
            match made {
                #[cfg(feature = "webgpu")]
                Ok(device) => init.target.set_device(device),
                #[cfg(not(feature = "webgpu"))]
                Ok(_) => {}
                Err(error) => init.failure = Some(error),
            }
        }
    }
}

/// What a session renders into, before its thread builds it.
pub(crate) enum TargetInit {
    Offscreen(Option<Device>),
    Surface { surface: Surface, device: Option<Device> },
    Borrowed { ring: BorrowedRing, device: Device, producer: SyncKind },
    Exported { ring: ExportedRing, device: Device, kind: ExportKind, producer: SyncKind },
}

impl TargetInit {
    /// The target on the session's lease of the host's device, and the
    /// lease, as [`Device::lease`] makes them.
    pub(crate) fn lease(self) -> (TargetInit, DeviceLease) {
        let lease_some = |device: Option<Device>| match device.map(Device::lease) {
            Some((device, lease)) => (Some(device), lease),
            None => (None, DeviceLease::default()),
        };
        match self {
            TargetInit::Offscreen(device) => {
                let (device, lease) = lease_some(device);
                (TargetInit::Offscreen(device), lease)
            }
            TargetInit::Surface { surface, device } => {
                let (device, lease) = lease_some(device);
                (TargetInit::Surface { surface, device }, lease)
            }
            TargetInit::Borrowed { ring, device, producer } => {
                let (device, lease) = device.lease();
                (TargetInit::Borrowed { ring, device, producer }, lease)
            }
            TargetInit::Exported { ring, device, kind, producer } => {
                let (device, lease) = device.lease();
                (TargetInit::Exported { ring, device, kind, producer }, lease)
            }
        }
    }
}

/// Builds the session's renderer into `slot`, on the thread that runs the
/// session's work, before any other work.
pub(crate) fn build(shared: &Arc<SessionShared>, slot: &Slot) {
    let Some(Ok(init)) = shared.state.lock().init.take().map(<Box<dyn Any + Send>>::downcast::<Init>) else {
        return;
    };
    let _call = shared.enter_call();
    let mut outbox = Outbox::default();
    let gate = shared.gate.lock();
    let worker = Worker::new(Arc::clone(shared), *init, &mut outbox);
    *slot.borrow_mut() = Some(worker);
    drop(gate);
    shared.wait_for_senders();
    outbox.deliver();
}

/// The renderer and what it draws into.
struct Gfx {
    renderer: Renderer,
    target: LocalTarget,
    /// The target's backend, which the renderer draws with.
    backend: Rc<dyn RendererBackend>,
    glyph_cache: Arc<GlyphCache>,
}

impl Gfx {
    /// How many graphics objects a `Gfx` holds: its target, and its
    /// renderer's, which the renderer makes when it first draws.
    const OBJECTS: u32 = 2;

    fn new(init: Init) -> Result<Gfx, Error> {
        let Init { target, size, scale_factor, glyph_cache, tile_cache_enabled, failure } = init;
        if let Some(failure) = failure {
            return Err(failure);
        }
        let pixel_ratio = scale_factor as f32;
        let size = physical_size(size, pixel_ratio);
        let target = match target {
            TargetInit::Offscreen(device) => LocalTarget::offscreen(device, size)?,
            TargetInit::Surface { surface, device } => LocalTarget::surface(surface, device, size)?,
            TargetInit::Borrowed { ring, device, producer } => LocalTarget::borrowed(device, ring, producer)?,
            TargetInit::Exported { ring, device, kind, producer } => {
                LocalTarget::exported(device, &ring, kind, size, producer)?
            }
        };
        let backend = target.backend();
        let mut renderer = Renderer::with_glyph_cache(Rc::clone(&backend), pixel_ratio, None, Arc::clone(&glyph_cache));
        renderer.set_tile_cache_enabled(tile_cache_enabled);
        set_tile_worker_transport(&mut renderer);
        Ok(Gfx { renderer, target, backend, glyph_cache })
    }

    fn scope(&self) -> BackendScope<'_> {
        BackendScope::new(&*self.backend, ScopeType::Explicit)
    }

    /// Whether the target can take a frame now: its window, if any, has an
    /// area to present to, and it begins a frame, which a surface needs a
    /// texture for.
    fn ready_for_frame(&self) -> bool {
        if !self.target.window_has_area() {
            return false;
        }
        let _scope = self.scope();
        self.target.frames().is_some_and(|frames| frames.begin_frame().is_some())
    }

    /// Makes the renderer again on the target's backend, which replaced the
    /// one it drew with, with what the session set on it.
    fn renew_renderer(&mut self, events: &RendererEvents) {
        self.backend = self.target.backend();
        let pixel_ratio = self.renderer.get_pixel_ratio();
        let mut renderer =
            Renderer::with_glyph_cache(Rc::clone(&self.backend), pixel_ratio, None, Arc::clone(&self.glyph_cache));
        renderer.set_tile_cache_enabled(self.renderer.get_tile_cache_enabled());
        renderer.set_event_queue(Some(events.clone()));
        set_tile_worker_transport(&mut renderer);
        self.renderer = renderer;
    }
}

/// In the browser, puts the renderer's tile workers in the instance's Web
/// Workers. Elsewhere they stay on the background threads.
fn set_tile_worker_transport(renderer: &mut Renderer) {
    #[cfg(target_family = "wasm")]
    if let Some(transport) = crate::web::tile_worker_transport() {
        renderer.set_tile_worker_transport(transport);
    }
    #[cfg(not(target_family = "wasm"))]
    let _ = renderer;
}

/// A host's frame demand on its way, and the last command sent before it.
struct PendingDemand {
    demand: FrameDemand,
    sequence: u64,
    /// The demand's place among the demands that reached the session.
    serial: u64,
}

/// The demand that renders the map's still image while the image waits for
/// a frame. Its result has no token.
struct StillDemand {
    /// The update that the demand tried and drew nothing of, since a
    /// resource that the image needs had yet to load. The demand waits for
    /// a newer one.
    tried: Option<u64>,
}

/// A frame that the worker rendered, or tried to, and has yet to publish.
struct RenderedFrame {
    result: FrameResult,
    /// The renderer's events of the frame.
    events: Vec<RendererEvent>,
    /// The ring slot that the frame rendered into, with what the host may
    /// acquire of it.
    slot: Option<(usize, Option<Published>)>,
}

/// Work that waits for the work sent before it: resizes that haven't
/// landed, and demands that wait for the map's commands.
struct Behind {
    /// How many of the session's resizes must land first.
    resizes: u64,
    /// How many demands reached the session before the work.
    demands: u64,
    task: Task,
}

/// The texture of a borrowed ring that the next frame renders into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RingPick {
    /// The target isn't a ring.
    None,
    /// The ring's texture at this slot, which is now marked as rendering.
    Slot(usize),
    /// The host holds a frame in every texture it may render into.
    Full,
    /// No texture may be rendered into again.
    Quarantined,
}

/// What the renderer reported about the frame it drew.
struct FrameReport {
    mode: RenderMode,
    needs_repaint: bool,
}

/// A render session on its thread.
pub(crate) struct Worker {
    shared: Arc<SessionShared>,
    /// `None` once the session failed to build it, or let go of it.
    gfx: Option<Gfx>,
    /// The scale factor that the renderer draws at.
    scale_factor: f64,
    reader: Option<UpdateReader>,
    /// The map's queue for the renderer's events.
    map_events: Option<RendererEvents>,
    /// The renderer's own queue, which the session reads before it passes
    /// the events on to the map.
    renderer_events: RendererEvents,
    update_scheduled: Arc<AtomicBool>,
    /// The sequence number of the map's last command that ran.
    map_applied: u64,
    /// What the map publishes, from the bind on: the last command that ran.
    map_published: Option<Arc<MapPublished>>,
    /// The demands in the order they came, each waiting until the map ran
    /// the commands sent before it.
    queue: VecDeque<PendingDemand>,
    /// The demands that wait for a newer update than the one they found.
    waiting: Vec<(PendingDemand, u64)>,
    /// The demands that wait for a borrowed ring to free a texture, ahead of
    /// the queue.
    parked: VecDeque<PendingDemand>,
    /// The textures of a borrowed ring.
    ring: Option<BorrowedRing>,
    /// The update generation and session generation of the last render.
    rendered_update: u64,
    rendered_generation: u64,
    /// The demand of the map's still image, while the image waits for a
    /// frame.
    still: Option<StillDemand>,
    /// The `still_image_request_id` of the last still image that the
    /// session rendered.
    still_rendered: u64,
    /// The `still_image_request_id` of the still image that a resource error
    /// on its way to the map fails. The session draws no frame for it.
    still_failing: Cell<u64>,
    /// Whether the target holds a frame since it attached or resized.
    has_frame: bool,
    /// The sources of the last render.
    sources: Option<ImmutableVector<dyn source_impl::Impl>>,
    torn_down: bool,
    /// Whether the map refused to bind the session, which then never gets
    /// an update to render.
    unbound: bool,
    /// Whether the session ended: its thread has no more work for it.
    pub(crate) finished: bool,
    /// Counts the resizes that landed, of those that the session sent.
    resizes_landed: u64,
    /// Counts the host's demands that reached the session.
    demands_received: u64,
    /// The work that waits for the work sent before it, in the order sent.
    behind: VecDeque<Behind>,
    /// The target and backend that a teardown keeps while views name their
    /// sync objects, until the last view ends.
    viewed: Option<(LocalTarget, Rc<dyn RendererBackend>)>,
    /// The releases and replacements that wait for the host's reads of the
    /// textures that they let go of.
    settling: Vec<Settling>,
    /// Stops the thread that has the session's thread poll `settling`.
    ticker: Option<Arc<AtomicBool>>,
}

/// The longest that a release or replacement waits for the host's reads of
/// the textures that it lets go of.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the session's thread polls the host's reads.
#[cfg(not(target_family = "wasm"))]
const SETTLE_POLL: Duration = Duration::from_millis(2);

/// A release or replacement that waits for the host's reads.
struct Settling {
    reads: Box<dyn PendingReads>,
    deadline: Instant,
    done: SettleDone,
}

/// What runs once a release or replacement has settled, with its result.
type SettleDone = Box<dyn FnOnce(&mut Outbox, Result<(), Error>)>;

/// Starts a thread that has the session's thread poll the host's reads on
/// its later turns, until the returned flag stops it. It posts to the
/// session's loop, so a caller-driven session's host gets a wake for each
/// poll.
#[cfg(target_family = "wasm")]
fn start_ticker(run_loop: RunLoopHandle) -> Arc<AtomicBool> {
    // The browser's one thread polls in the pump's slices instead, which it
    // asks for meanwhile.
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let deadline = Instant::now() + SETTLE_TIMEOUT * 2;
    mln_web_host::pump::add(move || {
        if stopped.load(Ordering::SeqCst) || Instant::now() >= deadline {
            return false;
        }
        post(&run_loop, Worker::poll_settling);
        mln_web_host::pump::wake();
        true
    })
    .detach();
    stop
}

#[cfg(not(target_family = "wasm"))]
fn start_ticker(run_loop: RunLoopHandle) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let deadline = Instant::now() + SETTLE_TIMEOUT * 2;
    let spawned = mln_util::thread::builder("RenderSessionSettle").spawn(move || {
        while !stopped.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(SETTLE_POLL);
            post(&run_loop, Worker::poll_settling);
        }
    });
    if spawned.is_err() {
        Log::error(Event::Render, "Can't start the thread that polls the host's reads of released textures");
    }
    stop
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.drop_gfx();
    }
}

fn target_lost() -> Error {
    Error::target_lost("the render session lost its renderer")
}

impl Worker {
    fn new(shared: Arc<SessionShared>, init: Init, outbox: &mut Outbox) -> Worker {
        let renderer_events = RendererEvents::new();
        let run_loop = shared.run_loop.clone();
        renderer_events.set_waker(Some(Arc::new(move || {
            post(&run_loop, |worker, _| {
                worker.forward_events();
            });
        })));
        // A session that a detach or abandon reached first needs no renderer.
        let attaching = shared.state.lock().lifecycle == RenderSessionState::Attaching;
        let scale_factor = init.scale_factor;
        let ring = match &init.target {
            TargetInit::Borrowed { ring, .. } => Some(ring.clone()),
            _ => None,
        };
        let built = attaching.then(|| Gfx::new(init));
        let gfx = match built {
            Some(Ok(mut gfx)) => {
                gfx.renderer.set_event_queue(Some(renderer_events.clone()));
                let mut state = shared.state.lock();
                state.graphics_objects = Gfx::OBJECTS - 1;
                state.abandon_keeps_graphics = gfx.target.device().abandon_keeps_graphics();
                drop(state);
                Some(gfx)
            }
            Some(Err(error)) => {
                let attach = {
                    let mut state = shared.state.lock();
                    if state.lifecycle == RenderSessionState::Attaching {
                        state.lifecycle = RenderSessionState::Failed;
                    }
                    state.failure = Some(error.clone());
                    state.attach.take()
                };
                if let Some(attach) = attach {
                    outbox.resolve(attach, Err(error));
                }
                None
            }
            None => None,
        };
        Worker {
            shared,
            gfx,
            scale_factor,
            reader: None,
            map_events: None,
            renderer_events,
            update_scheduled: Arc::new(AtomicBool::new(false)),
            map_applied: 0,
            map_published: None,
            queue: VecDeque::new(),
            waiting: Vec::new(),
            parked: VecDeque::new(),
            ring,
            rendered_update: 0,
            rendered_generation: 0,
            still: None,
            still_rendered: 0,
            still_failing: Cell::new(0),
            has_frame: false,
            sources: None,
            torn_down: false,
            unbound: false,
            finished: false,
            resizes_landed: 0,
            demands_received: 0,
            behind: VecDeque::new(),
            viewed: None,
            settling: Vec::new(),
            ticker: None,
        }
    }

    /// The completion of the work `id`, unless an abandon completed it.
    pub(crate) fn take_work<T: Send + 'static>(&self, id: WorkId) -> Option<Resolver<T>> {
        self.shared.take_work(id)
    }

    // MARK: - The map

    /// The map bound the session in its command `sequence`: from now on the
    /// session draws its updates and reports to it.
    pub(crate) fn bound(
        &mut self,
        sequence: u64,
        reader: UpdateReader,
        events: RendererEvents,
        published: Arc<MapPublished>,
        outbox: &mut Outbox,
    ) {
        if self.torn_down {
            return;
        }
        self.map_applied = self.map_applied.max(sequence);
        self.map_published = Some(published);
        let scheduled = Arc::clone(&self.update_scheduled);
        let run_loop = self.shared.run_loop.clone();
        // A caller-driven session's thread runs only when the host services
        // it, which the host may do only once it knows to draw, so the map's
        // thread tells it of the update.
        let redraw = (self.shared.driver == Driver::CallerThread).then(|| Arc::downgrade(&self.shared));
        reader.set_waker(Some(Arc::new(move || {
            if let Some(wake) = redraw.as_ref().and_then(Weak::upgrade).and_then(|shared| shared.take_redraw_wake()) {
                wake();
            }
            if !scheduled.swap(true, Ordering::SeqCst) {
                post(&run_loop, Worker::on_update);
            }
        })));
        self.reader = Some(reader);
        self.map_events = Some(events);
        let attach = {
            let mut state = self.shared.state.lock();
            if state.lifecycle == RenderSessionState::Attaching {
                state.lifecycle = RenderSessionState::Attached;
            }
            state.attach.take()
        };
        if let Some(attach) = attach {
            outbox.resolve(attach, Ok(()));
        }
        // A new session has the map's latest update to draw.
        if self.gfx.is_some() && self.latest().is_some() {
            self.wake_for_redraw(outbox);
        }
        // The demands that came while the session attached wait for the
        // attach to complete, which the outbox delivers once this task ends,
        // so they render in a task of their own.
        post(&self.shared.run_loop, Worker::pump);
    }

    /// The map couldn't bind the session.
    pub(crate) fn bind_failed(&mut self, error: Error, outbox: &mut Outbox) {
        let attach = {
            let mut state = self.shared.state.lock();
            if state.lifecycle == RenderSessionState::Attaching {
                state.lifecycle = RenderSessionState::Failed;
                state.failure = Some(error.clone());
            }
            state.attach.take()
        };
        self.unbound = true;
        self.drop_gfx();
        if let Some(attach) = attach {
            outbox.resolve(attach, Err(error));
        }
        self.end_demands(FrameDisposition::Failed, outbox);
        self.run_queued(outbox);
    }

    /// The map ran its command `sequence`, and published what it changed.
    pub(crate) fn map_applied(&mut self, sequence: u64, outbox: &mut Outbox) {
        self.map_applied = self.map_applied.max(sequence);
        self.pump(outbox);
    }

    /// The map published an update.
    fn on_update(&mut self, outbox: &mut Outbox) {
        self.update_scheduled.store(false, Ordering::SeqCst);
        if self.gfx.is_some() && self.latest().is_some_and(|update| update.generation != self.rendered_update) {
            self.wake_for_redraw(outbox);
        }
        self.pump(outbox);
    }

    /// Tells the host that the map has something new to draw, unless it was
    /// told since its last demand, or the session has no target.
    fn wake_for_redraw(&self, outbox: &mut Outbox) {
        if let Some(wake) = self.shared.take_redraw_wake() {
            outbox.push(move || wake());
        }
    }

    fn latest(&self) -> Option<Update> {
        self.reader.as_ref()?.latest()
    }

    fn latest_generation(&self) -> u64 {
        self.reader.as_ref().map_or(0, UpdateReader::generation)
    }

    /// Passes the renderer's events on to the map.
    fn forward_events(&mut self) {
        let events = self.renderer_events.take();
        self.forward(events);
    }

    fn forward(&self, events: Vec<RendererEvent>) {
        // The map fails the still image that waits when it takes a resource
        // error, so a frame for that image drawn before the map says so would
        // show a failed image.
        if events.iter().any(|event| matches!(event, RendererEvent::ResourceError(_))) {
            self.still_failing.set(self.pending_still_image());
        }
        if let Some(map_events) = &self.map_events {
            for event in events {
                map_events.push(event);
            }
        }
    }

    // MARK: - Frames

    /// Takes a host's demand, which supersedes the demands with its mode on
    /// their way unless it forces a frame.
    pub(crate) fn demand(&mut self, demand: FrameDemand, sequence: u64, outbox: &mut Outbox) {
        if self.torn_down {
            let result = FrameResult::unrendered(Some(demand.token), FrameDisposition::TargetNotReady, 0);
            return self.publish(&result, outbox);
        }
        if demand.mode != DemandMode::Force {
            let latest = self.latest_generation();
            let (superseded, kept): (Vec<_>, Vec<_>) =
                self.queue.drain(..).partition(|pending| pending.demand.mode == demand.mode);
            self.queue = kept.into();
            let (waiting_superseded, waiting): (Vec<_>, Vec<_>) =
                self.waiting.drain(..).partition(|(pending, _)| pending.demand.mode == demand.mode);
            self.waiting = waiting;
            let superseded = superseded.into_iter().chain(waiting_superseded.into_iter().map(|(pending, _)| pending));
            for pending in superseded {
                let result = FrameResult::unrendered(Some(pending.demand.token), FrameDisposition::Superseded, latest);
                self.publish(&result, outbox);
            }
        }
        self.demands_received += 1;
        self.queue.push_back(PendingDemand { demand, sequence, serial: self.demands_received });
        self.pump(outbox);
    }

    /// Evaluates the demands whose commands ran, then the demand of a still
    /// image that the map waits for, then the demands whose update came: an
    /// update that the image's frame showed is no news to them.
    fn pump(&mut self, outbox: &mut Outbox) {
        if self.torn_down {
            return;
        }
        if self.reader.is_none() {
            // A demand that came after a failed bind ends at once.
            if self.unbound {
                self.end_demands(FrameDisposition::Failed, outbox);
            }
            return self.run_queued(outbox);
        }
        while let Some(pending) = self.parked.pop_front() {
            if let Some(pending) = self.evaluate(pending, outbox) {
                self.parked.push_front(pending);
                self.run_queued(outbox);
                return self.serve_still(outbox);
            }
        }
        self.run_queued(outbox);
        self.serve_still(outbox);
        let latest = self.latest_generation();
        let retargeting = self.retargeting();
        let (updated, waiting): (Vec<_>, Vec<_>) =
            self.waiting.drain(..).partition(|(_, seen)| !retargeting && latest > *seen);
        self.waiting = waiting;
        for (pending, _) in updated {
            if let Some(pending) = self.evaluate(pending, outbox) {
                self.parked.push_back(pending);
            }
        }
    }

    /// Whether a ring replacement is on its way, which gives the demands
    /// that wait a new target: they render to it once it lands.
    fn retargeting(&self) -> bool {
        self.shared.state.lock().pending_retargets != 0
    }

    /// Runs the demands whose map commands ran and the work that waits
    /// behind earlier work, in the order they came, until each has to wait:
    /// a demand for the map's commands or for a parked demand, and other
    /// work for a resize to land or a demand sent before it that waits for
    /// the map's commands. Work doesn't wait for a ring to free a texture,
    /// as a parked demand and those behind it do.
    fn run_queued(&mut self, outbox: &mut Outbox) {
        loop {
            self.run_ready(outbox);
            // The map tells the session of the commands that the demands
            // left wait for. One may have run before it could.
            let awaited = self
                .queue
                .iter()
                .map(|pending| pending.sequence)
                .filter(|sequence| *sequence > self.map_applied)
                .min()
                .unwrap_or(u64::MAX);
            self.shared.awaited_command.store(awaited, Ordering::SeqCst);
            let applied = self.map_published.as_ref().map_or(0, |published| published.applied());
            if awaited == u64::MAX || applied < awaited {
                return;
            }
            self.map_applied = applied;
        }
    }

    /// Runs what [`run_queued`](Self::run_queued) runs, with the map's
    /// commands that the session knows ran.
    fn run_ready(&mut self, outbox: &mut Outbox) {
        loop {
            let front = self.queue.front().map(|pending| (pending.serial, pending.sequence <= self.map_applied));
            let work_first =
                self.behind.front().is_some_and(|behind| front.is_none_or(|(serial, _)| behind.demands < serial));
            let evaluable = !self.torn_down && self.reader.is_some() && self.parked.is_empty();
            if !work_first && evaluable && front.is_some_and(|(_, ready)| ready) {
                if let Some(pending) = self.queue.pop_front()
                    && let Some(pending) = self.evaluate(pending, outbox)
                {
                    self.parked.push_back(pending);
                }
                continue;
            }
            let waits = |behind: &Behind| {
                behind.resizes > self.resizes_landed
                    || self
                        .queue
                        .iter()
                        .any(|pending| pending.serial <= behind.demands && pending.sequence > self.map_applied)
            };
            match self.behind.front() {
                Some(behind) if !waits(behind) => {
                    if let Some(behind) = self.behind.pop_front() {
                        (behind.task)(self, outbox);
                    }
                }
                _ => return,
            }
        }
    }

    /// Renders the demand, or ends it, or has it wait for an update. Returns
    /// it when it waits for the ring to free a texture.
    fn evaluate(&mut self, pending: PendingDemand, outbox: &mut Outbox) -> Option<PendingDemand> {
        let token = Some(pending.demand.token);
        let latest = self.latest();
        let latest_generation = latest.as_ref().map_or(0, |update| update.generation);
        if self.gfx.is_none() {
            let failed = self.shared.state.lock().lifecycle == RenderSessionState::Failed;
            let disposition = if failed { FrameDisposition::Failed } else { FrameDisposition::TargetNotReady };
            self.publish(&FrameResult::unrendered(token, disposition, latest_generation), outbox);
            return None;
        }
        let changed = latest_generation != self.rendered_update || self.generation() != self.rendered_generation;
        match latest {
            Some(update) if pending.demand.mode == DemandMode::Force || changed => {
                // The texture is picked and marked in one step, so the host
                // can't acquire it in between.
                let slot = match self.pick_slot() {
                    RingPick::Full => return Some(pending),
                    RingPick::Quarantined => {
                        let result =
                            FrameResult::unrendered(token, FrameDisposition::TargetNotReady, latest_generation);
                        self.publish(&result, outbox);
                        return None;
                    }
                    RingPick::None => None,
                    RingPick::Slot(slot) => Some(slot),
                };
                let frame = self.render(token, &update, slot);
                self.publish_rendered(frame, outbox);
            }
            _ if pending.demand.mode == DemandMode::WaitForUpdate => self.waiting.push((pending, latest_generation)),
            _ => self.publish(&FrameResult::unrendered(token, FrameDisposition::NoUpdate, latest_generation), outbox),
        }
        None
    }

    /// The map's still image changed: one waits for a frame, or none does.
    pub(crate) fn still_image_changed(&mut self, outbox: &mut Outbox) {
        self.serve_still(outbox);
    }

    /// The still image that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or 0.
    fn pending_still_image(&self) -> u64 {
        self.map_published.as_ref().map_or(0, |published| published.pending_still_image())
    }

    /// Serves the map's still image: demands a frame for it while it waits
    /// for one, and renders the demand once the target can take a frame and
    /// the map has an update that it hasn't tried. The demand finishes with
    /// the image's frame. One that draws nothing, because a resource that
    /// the image needs is still loading, waits for the map's next update,
    /// and one that finds the ring full parks until the host releases a
    /// frame. A host's demands neither replace nor wait for it. Ending the
    /// demands on their way ends it too, until the session can render again:
    /// a barrier demands the image again once it ran, a new target once it
    /// lands, and the next session once it attaches.
    fn serve_still(&mut self, outbox: &mut Outbox) {
        let pending = self.pending_still_image();
        if pending == 0 {
            // The image completed or failed meanwhile.
            if self.still.take().is_some() {
                let result = FrameResult::unrendered(None, FrameDisposition::NoUpdate, self.latest_generation());
                self.publish(&result, outbox);
            }
            return;
        }
        if self.still.is_none() {
            if pending <= self.still_rendered || !self.can_render() {
                return;
            }
            self.still = Some(StillDemand { tried: None });
        }
        // The demand ends once the map fails the image.
        if pending == self.still_failing.get() {
            return;
        }
        let Some(update) = self.latest() else { return };
        // The update that asks for the image comes before the map says that
        // the image waits.
        let asks = update.parameters.still_image_request && update.parameters.still_image_request_id == pending;
        let tried = self.still.as_ref().and_then(|still| still.tried);
        if !asks
            || tried == Some(update.generation)
            || self.retargeting()
            || !self.gfx.as_ref().is_some_and(Gfx::ready_for_frame)
        {
            return;
        }
        let slot = match self.pick_slot() {
            RingPick::Full => return,
            RingPick::Quarantined => {
                self.still = None;
                let result = FrameResult::unrendered(None, FrameDisposition::TargetNotReady, update.generation);
                return self.publish(&result, outbox);
            }
            RingPick::None => None,
            RingPick::Slot(slot) => Some(slot),
        };
        let frame = self.render(None, &update, slot);
        // A tile that failed during the frame counts as loaded, so the frame
        // drew the map without it, but the map fails the image on the error.
        let fails = frame.events.iter().any(|event| matches!(event, RendererEvent::ResourceError(_)));
        match frame.result.disposition {
            FrameDisposition::Rendered if fails => {
                self.still_failing.set(pending);
                if let Some((slot, _)) = frame.slot {
                    self.end_slot(slot, None);
                }
                self.forward(frame.events);
            }
            FrameDisposition::Rendered => {
                self.still = None;
                self.still_rendered = pending;
                self.publish_rendered(frame, outbox);
            }
            disposition => {
                // A frame that a surface dropped is tried again.
                if disposition == FrameDisposition::NoUpdate
                    && let Some(still) = self.still.as_mut()
                {
                    still.tried = Some(update.generation);
                }
                if let Some((slot, published)) = frame.slot {
                    self.end_slot(slot, published);
                }
                self.forward(frame.events);
            }
        }
    }

    /// Whether the session can render a frame for a demand of its own: it
    /// attached, has a target, and a ring with a texture it may render into.
    fn can_render(&self) -> bool {
        if self.torn_down || self.reader.is_none() || self.gfx.is_none() {
            return false;
        }
        let state = self.shared.state.lock();
        state.lifecycle == RenderSessionState::Attached
            && state.has_target
            && (state.ring.is_empty() || !state.ring.iter().all(|slot| slot.quarantined))
    }

    fn generation(&self) -> u64 {
        self.shared.state.lock().generation
    }

    /// Renders `update` for the demand `token`, or for the still image's
    /// demand when it has none, into the ring's `slot`, which
    /// [`pick_slot`](Self::pick_slot) marked, or into a target without a
    /// ring. Returns the frame, which
    /// [`publish_rendered`](Self::publish_rendered) publishes. A still map draws no frame until everything that it shows
    /// has loaded, and a surface without a texture to render into takes no
    /// frame. A frame that a surface drops, having had no image for it after
    /// all, finishes as not ready, and the next demand draws the map again.
    /// Only the still image's own frame completes the image.
    fn render(&mut self, token: Option<u64>, update: &Update, slot: Option<usize>) -> RenderedFrame {
        let ready = self.gfx.as_ref().is_some_and(Gfx::ready_for_frame);
        let Some(gfx) = self.gfx.as_mut().filter(|_| ready) else {
            if let Some(slot) = slot {
                self.end_slot(slot, None);
            }
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return RenderedFrame { result, events: Vec::new(), slot: None };
        };
        if let Some(slot) = slot
            && gfx.target.select_slot(slot).is_err()
        {
            self.end_slot(slot, None);
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return RenderedFrame { result, events: Vec::new(), slot: None };
        }
        let handoff = {
            let _scope = BackendScope::new(&*gfx.backend, ScopeType::Explicit);
            gfx.target.frames().and_then(|frames| gfx.renderer.render(&update.parameters, frames))
        };
        {
            let mut state = self.shared.state.lock();
            state.graphics_objects = state.graphics_objects.max(Gfx::OBJECTS);
        }
        let mut events = self.renderer_events.take();
        if token.is_some() {
            for event in &mut events {
                if let RendererEvent::RenderFrameFinished { still_image_request_id, .. } = event {
                    *still_image_request_id = 0;
                }
            }
        }
        if gfx.target.is_presenting() && matches!(handoff, Some(Handoff::Dropped)) {
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return RenderedFrame { result, events, slot: None };
        }
        let projection = gfx.renderer.frame_projection();
        self.rendered_update = update.generation;
        self.rendered_generation = self.generation();
        self.sources = Some(Arc::clone(&update.parameters.sources));
        let report = events.iter().rev().find_map(|event| match event {
            RendererEvent::RenderFrameFinished { mode, needs_repaint, .. } => Some(FrameReport {
                mode: match mode {
                    mln_renderer::RenderMode::Full => RenderMode::Full,
                    mln_renderer::RenderMode::Partial => RenderMode::Partial,
                },
                needs_repaint: *needs_repaint,
            }),
            _ => None,
        });
        let result = match report {
            Some(report) => {
                self.has_frame = true;
                // The session takes the frame's generation in
                // `publish_frame`.
                let (generation, frame_generation) = {
                    let state = self.shared.state.lock();
                    (state.generation, state.frame_generation + 1)
                };
                FrameResult {
                    disposition: FrameDisposition::Rendered,
                    token,
                    map_update_generation: update.generation,
                    generation,
                    frame_generation,
                    needs_repaint: report.needs_repaint,
                    view: Some(MapView::from_state(&update.parameters.transform_state, self.scale_factor)),
                    mode: report.mode,
                    projection,
                }
            }
            None => FrameResult::unrendered(token, FrameDisposition::NoUpdate, update.generation),
        };
        let slot = slot.map(|slot| {
            let published = match handoff {
                Some(Handoff::GivenBack { slot: given, sync }) if given == slot && result.view.is_some() => {
                    let device = self.gfx.as_ref().map(|gfx| gfx.target.device().clone());
                    let texture = self.ring.as_ref().zip(device).and_then(|(ring, device)| ring.texture(slot, &device));
                    Some(Published { result, sync, texture })
                }
                Some(Handoff::Queued { slot: given, sync }) if given == slot && result.view.is_some() => {
                    let texture = self.gfx.as_ref().and_then(|gfx| gfx.target.exported_texture(slot));
                    Some(Published { result, sync, texture })
                }
                _ => None,
            };
            (slot, published)
        });
        RenderedFrame { result, events, slot }
    }

    /// Publishes a frame: its result and ring slot, then the renderer's
    /// events of the frame, which go to the map after the result, so that a
    /// still image completes with its frame's result queued.
    fn publish_rendered(&self, frame: RenderedFrame, outbox: &mut Outbox) {
        self.publish_frame(frame.slot, &frame.result, outbox);
        self.forward(frame.events);
    }

    /// Queues a frame result for the host, and wakes it unless it was woken
    /// since its last drain.
    fn publish(&self, result: &FrameResult, outbox: &mut Outbox) {
        self.publish_frame(None, result, outbox);
    }

    /// Publishes a frame: ends the ring `slot` it rendered into, with what
    /// the host may acquire of it, and queues its result as `publish` does.
    /// The slot, the result and the session's frame generation change under
    /// one lock, so a snapshot shows a frame only once its result can be
    /// drained and, in a ring, its slot acquired.
    fn publish_frame(&self, slot: Option<(usize, Option<Published>)>, result: &FrameResult, outbox: &mut Outbox) {
        let wake = {
            let mut state = self.shared.state.lock();
            if let Some((slot, published)) = slot
                && let Some(entry) = state.ring.get_mut(slot)
            {
                entry.rendering = false;
                entry.frame = published;
            }
            state.results.push(*result);
            state.frame_generation = state.frame_generation.max(result.frame_generation);
            let wake = !state.frame_woken;
            state.frame_woken = true;
            wake.then(|| self.shared.frame_wake.clone()).flatten()
        };
        if let Some(wake) = wake {
            outbox.push(move || wake());
        }
    }

    /// Ends every demand on its way with `disposition`, the still image's
    /// included.
    pub(crate) fn end_demands(&mut self, disposition: FrameDisposition, outbox: &mut Outbox) {
        let latest = self.latest_generation();
        let pending: Vec<_> = self
            .parked
            .drain(..)
            .chain(self.queue.drain(..))
            .chain(self.waiting.drain(..).map(|(pending, _)| pending))
            .collect();
        for pending in pending {
            self.publish(&FrameResult::unrendered(Some(pending.demand.token), disposition, latest), outbox);
        }
        if self.still.take().is_some() {
            self.publish(&FrameResult::unrendered(None, disposition, latest), outbox);
        }
    }

    /// Ends every demand on its way as a barrier does, then demands the
    /// still image again, if one waits, once the barrier ran.
    pub(crate) fn barrier(&mut self, outbox: &mut Outbox) {
        self.end_demands(FrameDisposition::NoUpdate, outbox);
        post(&self.shared.run_loop, Worker::still_image_changed);
    }

    // MARK: - Borrowed rings

    /// Picks the ring's texture that the next frame renders into, and marks
    /// it as rendering: one that holds no frame, or else the one with the
    /// oldest frame that the host didn't acquire.
    fn pick_slot(&self) -> RingPick {
        let mut state = self.shared.state.lock();
        if state.ring.is_empty() {
            return RingPick::None;
        }
        if state.ring.iter().all(|slot| slot.quarantined) {
            return RingPick::Quarantined;
        }
        // A frame may draw into a texture whose host's reads a release
        // waits for.
        if !self.settling.is_empty() {
            return RingPick::Full;
        }
        let free = state.ring.iter().enumerate().filter(|(_, slot)| !slot.busy());
        let index = free
            .clone()
            .find(|(_, slot)| slot.frame.is_none())
            .or_else(|| {
                free.min_by_key(|(_, slot)| slot.frame.as_ref().map_or(0, |frame| frame.result.frame_generation))
            })
            .map(|(index, _)| index);
        let Some(index) = index else { return RingPick::Full };
        let slot = &mut state.ring[index];
        slot.rendering = true;
        slot.frame = None;
        RingPick::Slot(index)
    }

    /// Ends a frame rendered into the ring's `slot`, which then holds
    /// `published` for the host to acquire.
    fn end_slot(&self, slot: usize, published: Option<Published>) {
        let mut state = self.shared.state.lock();
        if let Some(entry) = state.ring.get_mut(slot) {
            entry.rendering = false;
            entry.frame = published;
        }
    }

    /// The host gave the ring's `slot` back with its `consumer` sync: the
    /// next frame rendered into it waits for that. A sync the target can't
    /// wait for fails the session, and the slot is never rendered into
    /// again, since the host's GPU may still read it. A slot that comes back
    /// after the ring let go of its textures has nothing to wait in, so it
    /// is only quarantined, unless its reads are done.
    pub(crate) fn release_slot(&mut self, slot: usize, consumer: ConsumerSync, outbox: &mut Outbox) {
        let kind = consumer.sync.kind;
        let released = match self.gfx.as_ref() {
            Some(gfx) if gfx.target.ring_released() => {
                let mut state = self.shared.state.lock();
                if let Some(entry) = state.ring.get_mut(slot) {
                    entry.acquired = false;
                    entry.quarantined |= kind != SyncKind::CpuComplete;
                }
                drop(state);
                return self.pump(outbox);
            }
            Some(gfx) => {
                let _scope = gfx.scope();
                gfx.target.release_slot(slot, consumer)
            }
            None => Err(Error::target_lost("the render session lost its renderer")),
        };
        let failure = {
            let mut state = self.shared.state.lock();
            if let Some(entry) = state.ring.get_mut(slot) {
                entry.acquired = false;
                entry.quarantined |= released.is_err() && kind != SyncKind::CpuComplete && !entry.retired;
                entry.retired = false;
            }
            match &released {
                Err(error) if self.gfx.is_some() && state.lifecycle == RenderSessionState::Attached => {
                    state.lifecycle = RenderSessionState::Failed;
                    state.failure = Some(error.clone());
                    true
                }
                _ => false,
            }
        };
        if failure {
            self.end_demands(FrameDisposition::Failed, outbox);
            self.drop_gfx();
            return self.run_queued(outbox);
        }
        self.pump(outbox);
    }

    /// The host gave up the ring's `slot`, which the session quarantined.
    pub(crate) fn discard_slot(&mut self, slot: usize, outbox: &mut Outbox) {
        if let Some(gfx) = self.gfx.as_ref() {
            let _scope = gfx.scope();
            let _ = gfx.target.discard_slot(slot);
        }
        self.pump(outbox);
    }

    /// Renders into `ring` from now on, at `size` logical pixels, after the
    /// map resized. The ring's slots then hold no frames, and only the
    /// textures that were quarantined stay so.
    pub(crate) fn set_ring(
        &mut self,
        size: Size,
        scale_factor: f64,
        ring: &BorrowedRing,
        outbox: &mut Outbox,
    ) -> Result<Pending, Error> {
        self.set_scale_factor(scale_factor)?;
        let same_size = self.size() == size;
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        // The ring it has already: nothing changes, and the frames it holds
        // stay.
        if self.ring.as_ref() == Some(ring) && gfx.target.frames().is_some() && same_size {
            // The demands that wait keep waiting for an update past the
            // replacement's resize.
            let latest = self.latest_generation();
            for (_, seen) in &mut self.waiting {
                *seen = latest;
            }
            return Ok(None);
        }
        let backend = Rc::clone(&gfx.backend);
        let replaced = {
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            gfx.target.set_ring(ring.clone())
        };
        let pending = replaced?;
        let old = self.ring.replace(ring.clone());
        {
            let mut state = self.shared.state.lock();
            // The host's GPU may still read a texture it gave up, wherever it
            // is in the new ring.
            let quarantined: Vec<bool> = (0..state.ring.len())
                .map(|slot| {
                    old.as_ref().is_some_and(|old| {
                        state
                            .ring
                            .iter()
                            .enumerate()
                            .any(|(old_slot, entry)| entry.quarantined && ring.same_texture(slot, old, old_slot))
                    })
                })
                .collect();
            for (entry, quarantined) in state.ring.iter_mut().zip(quarantined) {
                entry.frame = None;
                entry.quarantined = quarantined;
            }
        }
        self.target_changed(size, Some(true));
        self.wake_for_redraw(outbox);
        // The demands that wait for an update find a new target, which they
        // render to.
        let waiting: Vec<_> = self.waiting.drain(..).map(|(pending, _)| pending).collect();
        self.parked.extend(waiting);
        self.pump(outbox);
        Ok(pending)
    }

    // MARK: - Work

    /// Runs `task` after the work sent before it: once `resizes` of the
    /// session's resizes have landed, and the demands that came before it
    /// and wait for the map's commands have rendered or ended.
    pub(crate) fn after_earlier_work(&mut self, resizes: u64, task: Task, outbox: &mut Outbox) {
        self.behind.push_back(Behind { resizes, demands: self.demands_received, task });
        self.run_queued(outbox);
    }

    fn resize_landed(&mut self, outbox: &mut Outbox) {
        self.resizes_landed += 1;
        self.run_queued(outbox);
    }

    /// The target's size in logical pixels.
    pub(crate) fn size(&self) -> Size {
        self.shared.state.lock().size
    }

    pub(crate) fn renderer(&mut self) -> Result<&mut Renderer, Error> {
        self.gfx.as_mut().map(|gfx| &mut gfx.renderer).ok_or_else(target_lost)
    }

    /// Resizes the target, and draws at `scale_factor`, after the map
    /// resized.
    pub(crate) fn resize(&mut self, size: Size, scale_factor: f64, outbox: &mut Outbox) -> Result<(), Error> {
        self.set_scale_factor(scale_factor)?;
        let gfx = self.gfx.as_ref().ok_or_else(target_lost)?;
        let physical = physical_size(size, scale_factor as f32);
        match gfx.target.exported_size() {
            // An exported ring whose images have the size already keeps
            // them, with the frames they hold and the slots quarantined.
            Some(current) if current == physical => {}
            Some(_) => {
                // The frames that the host holds keep their images, and every
                // slot gets a new one. The frames it didn't acquire go first,
                // in the same step as the snapshot, so that none is acquired
                // with an image that the resize lets go of.
                let held: Vec<usize> = {
                    let mut state = self.shared.state.lock();
                    for slot in state.ring.iter_mut().filter(|slot| !slot.acquired) {
                        slot.frame = None;
                    }
                    state.ring.iter().enumerate().filter(|(_, slot)| slot.acquired).map(|(index, _)| index).collect()
                };
                let resized = {
                    let _scope = gfx.scope();
                    gfx.target.resize_exported(physical, &held)
                };
                match resized {
                    // Each slot has an image that never reached the host: a
                    // slot that the host gave up is free, and one it holds
                    // has its old image retired.
                    Ok(true) => {
                        let mut state = self.shared.state.lock();
                        for slot in &mut state.ring {
                            if slot.acquired {
                                slot.retired = true;
                            } else {
                                slot.quarantined = false;
                            }
                        }
                    }
                    Ok(false) => {}
                    // The map has the new size already, which the old images
                    // can't show.
                    Err(error) => {
                        self.fail(error.clone(), outbox);
                        return Err(error);
                    }
                }
            }
            None => gfx.target.resize(physical),
        }
        self.target_changed(size, None);
        Ok(())
    }

    /// Fails the attached session with `error`, ending the demands on their
    /// way, and lets go of its renderer and target.
    fn fail(&mut self, error: Error, outbox: &mut Outbox) {
        let failed = {
            let mut state = self.shared.state.lock();
            let attached = self.gfx.is_some() && state.lifecycle == RenderSessionState::Attached;
            if attached {
                state.lifecycle = RenderSessionState::Failed;
                state.failure = Some(error);
            }
            attached
        };
        if failed {
            self.end_demands(FrameDisposition::Failed, outbox);
            self.drop_gfx();
            self.run_queued(outbox);
        }
    }

    /// Draws at `scale_factor` from the next frame on, which the map took
    /// too: the renderer lays out the tiles it has again.
    fn set_scale_factor(&mut self, scale_factor: f64) -> Result<(), Error> {
        if scale_factor != self.scale_factor {
            self.renderer()?.set_pixel_ratio(scale_factor as f32);
            self.scale_factor = scale_factor;
        }
        Ok(())
    }

    /// The target changed to `size`, and has a surface again when
    /// `has_target` says so.
    fn target_changed(&mut self, size: Size, has_target: Option<bool>) {
        self.has_frame = false;
        let mut state = self.shared.state.lock();
        state.size = size;
        state.scale_factor = self.scale_factor;
        state.generation += 1;
        if let Some(has_target) = has_target {
            state.has_target = has_target;
        }
    }

    /// Lets go of a surface session's surface, or of a ring's textures, and
    /// ends the demands on their way. Returns the host's reads of the
    /// textures, which the release waits for. The host may destroy the
    /// textures once it completes, so a new ring keeps nothing of the old
    /// one: no texture of it stays quarantined.
    pub(crate) fn release_target(&mut self, outbox: &mut Outbox) -> Result<Pending, Error> {
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        let backend = Rc::clone(&gfx.backend);
        let pending = {
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            gfx.target.release_surface()
        };
        self.has_frame = false;
        self.ring = None;
        {
            let mut state = self.shared.state.lock();
            state.has_target = false;
            for slot in &mut state.ring {
                slot.frame = None;
                slot.quarantined = false;
            }
        }
        self.end_demands(FrameDisposition::TargetNotReady, outbox);
        Ok(pending)
    }

    // MARK: - The host's reads of released textures

    /// Runs `done` once the host's `pending` reads end, or a second passes:
    /// at once when there are none, and else on a later turn of the
    /// session's thread, which polls them without blocking. Frames wait for
    /// them meanwhile.
    pub(crate) fn settle_then(
        &mut self,
        pending: Pending,
        done: impl FnOnce(&mut Outbox, Result<(), Error>) + 'static,
        outbox: &mut Outbox,
    ) {
        let Some(reads) = pending else { return done(outbox, Ok(())) };
        self.settling.push(Settling { reads, deadline: Instant::now() + SETTLE_TIMEOUT, done: Box::new(done) });
        if self.ticker.is_none() {
            self.ticker = Some(start_ticker(self.shared.run_loop.clone()));
        }
    }

    /// Polls the host's reads that releases and replacements wait for, and
    /// completes those whose reads ended or that waited long enough.
    fn poll_settling(&mut self, outbox: &mut Outbox) {
        let now = Instant::now();
        let (settled, settling): (Vec<_>, Vec<_>) = std::mem::take(&mut self.settling)
            .into_iter()
            .map(|mut settling| (settling.reads.poll(), settling))
            .partition(|(ended, settling)| *ended || now >= settling.deadline);
        self.settling = settling.into_iter().map(|(_, settling)| settling).collect();
        for (ended, settling) in settled {
            if !ended {
                Log::warning(
                    Event::Render,
                    "The host's reads of a borrowed texture didn't end within a second of its release; the session \
                     no longer waits for them",
                );
            }
            let Settling { reads, done, .. } = settling;
            drop(reads);
            done(outbox, Ok(()));
        }
        if self.settling.is_empty() {
            self.stop_ticker();
            self.pump(outbox);
        }
    }

    fn stop_ticker(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.store(true, Ordering::SeqCst);
        }
    }

    /// Ends the releases and replacements that wait for the host's reads,
    /// which the session no longer waits for, with `error`.
    fn end_settling(&mut self, error: &Error, outbox: &mut Outbox) {
        self.stop_ticker();
        for Settling { reads, done, .. } in std::mem::take(&mut self.settling) {
            drop(reads);
            done(outbox, Err(error.clone()));
        }
    }

    /// Presents to `surface` from now on, at `size` logical pixels, after
    /// the map resized. On failure the session keeps the surface that it
    /// had, if any, at the new size.
    pub(crate) fn set_surface(
        &mut self,
        size: Size,
        scale_factor: f64,
        surface: Surface,
        outbox: &mut Outbox,
    ) -> Result<(), Error> {
        let target = &self.gfx.as_ref().ok_or_else(target_lost)?.target;
        if target.is_offscreen() || target.is_borrowed() {
            return Err(Error::unsupported("only a surface session presents to another surface"));
        }
        self.set_scale_factor(scale_factor)?;
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        let physical = physical_size(size, scale_factor as f32);
        let backend = Rc::clone(&gfx.backend);
        let replaced = {
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            gfx.target.set_surface(surface, physical)
        };
        match replaced {
            Ok(true) => gfx.renew_renderer(&self.renderer_events),
            Ok(false) => {}
            Err(_) => gfx.target.resize(physical),
        }
        let replaced = replaced.map(drop);
        self.target_changed(size, replaced.is_ok().then_some(true));
        if replaced.is_ok() {
            self.wake_for_redraw(outbox);
            // A still image that waits for a target gets its demand again.
            post(&self.shared.run_loop, Worker::still_image_changed);
        }
        replaced
    }

    pub(crate) fn reduce_memory_use(&mut self) -> Result<(), Error> {
        self.renderer()?.reduce_memory_use();
        Ok(())
    }

    pub(crate) fn clear_data(&mut self, outbox: &mut Outbox) -> Result<(), Error> {
        self.renderer()?.clear_data();
        self.rendered_update = 0;
        // The renderer loads what the image needs again, once it tries.
        if let Some(still) = self.still.as_mut() {
            still.tried = None;
            post(&self.shared.run_loop, Worker::still_image_changed);
        }
        self.wake_for_redraw(outbox);
        Ok(())
    }

    pub(crate) fn dump_debug_logs(&mut self) -> Result<(), Error> {
        self.renderer()?.dump_debug_logs();
        Ok(())
    }

    pub(crate) fn read_texture(&mut self) -> Result<PremultipliedImage, Error> {
        let gfx = self.gfx.as_ref().ok_or_else(target_lost)?;
        if !gfx.target.is_offscreen() {
            return Err(Error::unsupported("a surface session can't read its frames back"));
        }
        if !self.has_frame {
            return Err(Error::invalid_state("the render session has no rendered frame since it attached or resized"));
        }
        let _scope = gfx.scope();
        gfx.target.read_image()
    }

    /// Runs `read` on the data of the clustered GeoJSON source `source_id`
    /// of the last render, with the children of its cluster `cluster_id`.
    pub(crate) fn cluster<T>(
        &mut self,
        source_id: &str,
        cluster_id: u32,
        read: impl FnOnce(&dyn GeoJSONData, Vec<feature::Feature<f64>>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        // A cluster's ID holds its zoom plus one in its low five bits, so an
        // ID without them names no cluster, though it would find the points
        // at zoom 0 as children. An index of no points has no clusters.
        const ZOOM_BITS: u32 = 0x1F;

        self.renderer()?;
        let source = self
            .sources
            .as_ref()
            .and_then(|sources| sources.iter().find(|source| source.base().id == source_id))
            .ok_or_else(|| Error::not_found(format!("the session's last render has no source named {source_id}")))?;
        let geojson = source
            .downcast_ref::<geojson_source_impl::Impl>()
            .ok_or_else(|| Error::invalid_argument(format!("source {source_id} is not a GeoJSON source")))?;
        if !geojson.get_options().cluster {
            return Err(Error::invalid_argument(format!("GeoJSON source {source_id} does not cluster")));
        }
        let data = geojson.get_data().and_then(|data| data.upgrade());
        let children = data
            .as_ref()
            .filter(|_| cluster_id & ZOOM_BITS != 0)
            .and_then(|data| data.get_children(cluster_id).ok())
            .filter(|children| !children.is_empty());
        match (data, children) {
            (Some(data), Some(children)) => read(&*data, children),
            _ => Err(Error::not_found(format!("the source has no cluster with ID {cluster_id}"))),
        }
    }

    // MARK: - Lifecycle

    /// Lets go of the map's updates and the graphics objects, and ends the
    /// demands on their way.
    pub(crate) fn tear_down(&mut self, outbox: &mut Outbox) {
        self.end_settling(&target_lost(), outbox);
        self.end_demands(FrameDisposition::TargetNotReady, outbox);
        self.torn_down = true;
        if let Some(reader) = self.reader.take() {
            reader.set_waker(None);
        }
        self.renderer_events.set_waker(None);
        self.forward_events();
        self.drop_gfx();
        self.map_events = None;
        self.sources = None;
        self.has_frame = false;
        let attach = self.shared.state.lock().attach.take();
        if let Some(attach) = attach {
            outbox.resolve(attach, Err(Error::target_lost("the render session ended before it attached")));
        }
    }

    /// Lets go of the renderer and its target: frees them, unless the
    /// session keeps them until the process exits. While a view is open on
    /// one of the ring's frames, the target and its backend stay until the
    /// last view ends.
    fn drop_gfx(&mut self) {
        let Some(mut gfx) = self.gfx.take() else { return };
        let (keep, viewed, abandoned) = {
            let mut state = self.shared.state.lock();
            state.graphics_objects = 0;
            let viewed = state.views != 0 && !state.keep_graphics;
            state.viewed_teardown |= viewed;
            (state.keep_graphics, viewed, state.lifecycle == RenderSessionState::Abandoned)
        };
        // The host's graphics objects may be gone, and the custom layers'
        // hosts with them, so they forget theirs instead of destroying them.
        if keep || abandoned {
            gfx.renderer.lose_custom_layers();
        }
        if keep {
            // The host's graphics objects behind them may be gone already.
            std::mem::forget(gfx);
        } else if viewed {
            let Gfx { renderer, target, backend, .. } = gfx;
            drop(renderer);
            self.viewed = Some((target, backend));
        }
    }

    /// The last view of the session's frames ended: lets go of the target
    /// that a teardown kept for it, and ends a session that waited for that.
    pub(crate) fn release_viewed(&mut self) {
        if self.viewed.take().is_some() && self.finished && self.shared.driver == Driver::CoreWorker {
            self.shared.run_loop.stop();
        }
    }

    /// Whether the session's thread is done with it: the session ended, and
    /// no view keeps its target.
    pub(crate) fn is_done(&self) -> bool {
        self.finished && self.viewed.is_none()
    }

    /// Lets go of the session on its thread, which runs no more of its work.
    pub(crate) fn close(mut self, outbox: &mut Outbox) {
        if !self.torn_down {
            self.tear_down(outbox);
        }
    }

    /// Detaches the session: it lets go of its graphics objects and its map,
    /// and ends, without waiting for the map's thread, which a host's
    /// completion may hold. The map unbinds the session before it binds
    /// another.
    pub(crate) fn detach(&mut self, outbox: &mut Outbox) {
        self.tear_down(outbox);
        let map = self.shared.state.lock().map.clone();
        if let Some(map) = map {
            map.unbind_session(self.shared.id);
        }
        self.finish(outbox);
    }

    /// Ends the session: it completes the detach and the releases, lets go
    /// of the map, and stops the session's thread.
    pub(crate) fn finish(&mut self, outbox: &mut Outbox) {
        let (waiters, map) = {
            let mut state = self.shared.state.lock();
            if state.lifecycle == RenderSessionState::Detaching {
                state.lifecycle = RenderSessionState::Detached;
            }
            (state.take_waiters(), state.map.take())
        };
        if let Some(map) = &map {
            map.release_session_claim(self.shared.id);
        }
        outbox.push(move || waiters.settle(&Ok(()), target_lost));
        outbox.push(move || drop(map));
        self.finished = true;
        // A target that views keep stops the loop once they end.
        if self.shared.driver == Driver::CoreWorker && self.viewed.is_none() {
            self.shared.run_loop.stop();
        }
    }
}
