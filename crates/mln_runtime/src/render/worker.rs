//! A render session's thread: its renderer, its frame demands, and the work
//! that the handle sends it.

#![cfg_attr(
    target_family = "wasm",
    expect(unreachable_code, unused_variables, reason = "the browser build renders into no borrowed ring")
)]

use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use mln_engine::text::GlyphCache;
use mln_engine::text::font_face_engine::FontFaceEngine;
use mln_engine::text::local_fonts::LocalGlyphs;
use mln_engine::{Registry, RenderMode, Renderer, RendererEvent, RendererEvents, Update, UpdateReader};
use mln_gfx::{ConsumerSync, GpuSync, Handoff, SyncKind};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::size::Size;
use mln_style::source::SourceData;
use mln_style::sources::{GeoJSONData, GeoJsonSourceType};
use mln_util::event_queue::Waker;
use mln_util::pool::Pool;
use mln_util::queue::Queue;

use super::frame::{DemandMode, FrameDemand, FrameDisposition, FrameResult};
use super::local::LocalTarget;
use super::session::{Published, RenderSessionState, SessionShared, WorkId, physical_size};
use super::target::{
    BorrowedRing, Device, DeviceLease, Driver, ExportKind, ExportedRing, SentSurface, Surface, SurfaceOverlay,
};
use crate::completion::Resolver;
use crate::log_target;
use crate::map::Published as MapPublished;
use crate::runtime::Outbox;

/// The work of the thread that runs a session, which owns the session's
/// worker once it is built.
pub(crate) type SessionQueue = Queue<Option<Worker>>;

/// Runs `task` on the session whose thread runs `queue`, after the work sent
/// before.
pub(crate) fn post(queue: &SessionQueue, task: impl FnOnce(&mut Worker, &mut Outbox) + Send + 'static) {
    queue.post(move |worker| run(worker, task));
}

/// Runs `task` on the session, then delivers what it sends to the host, with
/// the session unlocked: a driver call, which the delivery is part of. A
/// session that was abandoned lets go of its graphics objects first.
fn run(worker: &mut Option<Worker>, task: impl FnOnce(&mut Worker, &mut Outbox)) {
    let Some(worker) = worker.as_mut() else { return };
    let shared = Arc::clone(&worker.shared);
    let _call = shared.enter_call();
    let mut outbox = Outbox::default();
    {
        let _gate = shared.gate.lock();
        let ran = mln_util::panic::catch(|| {
            if !worker.torn_down && shared.state.lock().lifecycle == RenderSessionState::Abandoned {
                worker.tear_down(&mut outbox);
            }
            task(worker, &mut outbox);
        });
        if let Err(message) = ran {
            worker.fail_after_panic(&message, &mut outbox);
        }
    }
    shared.wait_for_senders();
    outbox.deliver();
}

/// Work for the session's thread.
pub(crate) type Task = Box<dyn FnOnce(&mut Worker, &mut Outbox) + Send>;

/// Tells the session's thread that a resize sent to the map has landed:
/// [`land`](Self::land) when the resize reaches the session, or on drop
/// when it never does.
pub(crate) struct ResizeLanding(Option<SessionQueue>);

impl ResizeLanding {
    pub(crate) fn new(queue: SessionQueue) -> Self {
        ResizeLanding(Some(queue))
    }

    pub(crate) fn land(mut self, worker: &mut Worker, outbox: &mut Outbox) {
        self.0 = None;
        worker.resize_landed(outbox);
    }
}

impl Drop for ResizeLanding {
    fn drop(&mut self) {
        if let Some(queue) = self.0.take() {
            post(&queue, Worker::resize_landed);
        }
    }
}

/// What a session's thread needs to build its renderer.
pub(crate) struct Init {
    pub(crate) target: TargetInit,
    pub(crate) size: Size,
    pub(crate) scale_factor: f64,
    pub(crate) glyph_cache: Arc<GlyphCache>,
    /// What the renderer loads a style's font faces with.
    pub(crate) font_face_engine: Arc<dyn FontFaceEngine>,
    /// What the renderer's tile workers lay out with.
    pub(crate) registry: Arc<Registry>,
    /// Whether the renderer keeps tiles that left the view.
    pub(crate) tile_cache_enabled: bool,
    /// How the renderer draws glyphs from local fonts.
    pub(crate) local_glyphs: LocalGlyphs,
    /// Why the session's device couldn't be made before the build: in the
    /// browser, whose WebGPU makes devices asynchronously.
    pub(crate) failure: Option<Error>,
    /// Where the renderer's background work runs: the runtime's pool.
    pub(crate) pool: Pool,
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
        let made = Device::request_web_gpu().await;
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
    Surface { surface: SentSurface, device: Option<Device> },
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
/// session's work, before any other work. The build takes `overlay` for
/// the session's first surface, if it builds.
pub(crate) fn build(shared: &Arc<SessionShared>, slot: &mut Option<Worker>, overlay: &mut Option<SurfaceOverlay>) {
    let Some(Ok(init)) = shared.state.lock().init.take().map(<Box<dyn Any + Send>>::downcast::<Init>) else {
        return;
    };
    let _call = shared.enter_call();
    let mut outbox = Outbox::default();
    let gate = shared.gate.lock();
    let worker = Worker::new(Arc::clone(shared), *init, overlay.take(), &mut outbox);
    *slot = Some(worker);
    drop(gate);
    shared.wait_for_senders();
    outbox.deliver();
}

/// What the session draws into, with the renderer that the target owns, and
/// what the session makes a renderer with.
struct Gfx {
    target: LocalTarget,
    glyph_cache: Arc<GlyphCache>,
    font_face_engine: Arc<dyn FontFaceEngine>,
    /// What the renderer's tile workers lay out with (`Init`).
    registry: Arc<Registry>,
    /// How the renderer draws glyphs from local fonts (`Init`).
    local_glyphs: LocalGlyphs,
    /// Where the renderer's background work runs (`Init`).
    pool: Pool,
}

impl Gfx {
    /// How many graphics objects a `Gfx` holds: its target, and its
    /// renderer's, which the renderer makes when it first draws.
    const OBJECTS: u32 = 2;

    /// A renderer on `init`'s target, which reports to `events`, with
    /// `overlay` drawn over the frames that a surface target presents.
    fn new(init: Init, overlay: Option<SurfaceOverlay>, events: &RendererEvents) -> Result<Gfx, Error> {
        let Init {
            target,
            size,
            scale_factor,
            glyph_cache,
            font_face_engine,
            registry,
            tile_cache_enabled,
            local_glyphs,
            failure,
            pool,
        } = init;
        if let Some(failure) = failure {
            return Err(failure);
        }
        let pixel_ratio = scale_factor as f32;
        let size = physical_size(size, pixel_ratio);
        let target = match target {
            TargetInit::Offscreen(device) => LocalTarget::offscreen(device, size)?,
            TargetInit::Surface { surface, device } => LocalTarget::surface(surface.receive()?, overlay, device, size)?,
            TargetInit::Borrowed { ring, device, producer } => LocalTarget::borrowed(device, ring, producer)?,
            TargetInit::Exported { ring, device, kind, producer } => {
                LocalTarget::exported(device, &ring, kind, size, producer)?
            }
        };
        let mut gfx = Gfx { target, glyph_cache, font_face_engine, registry, local_glyphs, pool };
        gfx.make_renderer(events, pixel_ratio, tile_cache_enabled);
        Ok(gfx)
    }

    /// Makes the target's renderer, on its backend, which reports to
    /// `events`, draws at `pixel_ratio` and keeps tiles that left the view
    /// when `tile_cache_enabled` says so.
    fn make_renderer(&mut self, events: &RendererEvents, pixel_ratio: f32, tile_cache_enabled: bool) {
        let Gfx { target, glyph_cache, font_face_engine, registry, local_glyphs, pool } = self;
        target.renderer_or(|gpu| {
            let mut renderer = Renderer::with_fonts(
                gpu,
                pixel_ratio,
                local_glyphs.clone(),
                Arc::clone(glyph_cache),
                Arc::clone(font_face_engine),
                Arc::clone(registry),
                invalidate(events),
                pool,
            );
            renderer.set_tile_cache_enabled(tile_cache_enabled);
            renderer.set_event_queue(Some(events.clone()));
            set_tile_workers(&mut renderer, registry);
            renderer
        });
    }
}

/// The renderer's waker: it asks the map for an update through `events`,
/// the queue the renderer reports to.
fn invalidate(events: &RendererEvents) -> Waker {
    let events = events.clone();
    Arc::new(move || events.push(RendererEvent::Invalidate))
}

/// In the browser, puts the renderer's tile workers in the instance's Web
/// Workers, which lay out with `registry`. Elsewhere they stay on the
/// background threads.
fn set_tile_workers(renderer: &mut Renderer, registry: &Arc<Registry>) {
    #[cfg(target_family = "wasm")]
    if let Some(workers) = crate::web::tile_workers(registry) {
        renderer.set_tile_workers(workers);
    }
    #[cfg(not(target_family = "wasm"))]
    let _ = (renderer, registry);
}

/// A host's frame demand on its way, and the last command sent before it.
struct PendingDemand {
    demand: FrameDemand,
    sequence: u64,
    /// The map's frame sample that the demand waits for, or 0.
    sample: u64,
    /// The demand's place among the demands that reached the session.
    serial: u64,
}

/// The demand that renders the map's still image while the image waits for
/// a frame. Its result has no token.
struct StillDemand {
    /// The `still_image_request_id` of the image.
    request: u64,
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

/// How a frame that was drawn for a still image ended.
#[derive(Clone, Copy, Debug)]
enum FrameOutcome {
    /// The frame went through its handoff.
    Handed,
    /// The frame was drawn for a host's demand.
    ForDemand,
    /// A surface dropped the frame.
    Dropped,
}

/// Takes the still image off the reports in `events` of a frame that ends
/// none: one drawn for a host's demand, or one that a surface `dropped`,
/// which has no picture to complete it with. A dropped frame that failed
/// keeps it, so that its error fails the still image it was drawn for.
fn disown_still(events: &mut [RendererEvent], outcome: FrameOutcome) {
    if matches!(outcome, FrameOutcome::Handed) {
        return;
    }
    for event in events {
        if let RendererEvent::RenderFrameFinished { still_image_request_id, error, .. } = event
            && (matches!(outcome, FrameOutcome::ForDemand) || error.is_none())
        {
            *still_image_request_id = 0;
        }
    }
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
    /// What the host draws over the frames presented to the next surface
    /// that the session makes ([`DriverWork::set_overlay`](super::DriverWork::set_overlay)).
    pub(crate) overlay: Option<SurfaceOverlay>,
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
    /// The map's last frame sample.
    map_sampled: u64,
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
    /// The token of the demand that the session is rendering, which a panic
    /// on the way ends with the others.
    rendering: Option<u64>,
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
    sources: Option<Arc<mln_style::SourceList>>,
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
    /// The target that a teardown keeps while views name their sync
    /// objects, until the last view ends.
    viewed: Option<LocalTarget>,
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
    fn new(shared: Arc<SessionShared>, init: Init, overlay: Option<SurfaceOverlay>, outbox: &mut Outbox) -> Worker {
        let renderer_events = RendererEvents::new();
        let queue = shared.queue.clone();
        renderer_events.set_waker(Some(Arc::new(move || {
            post(&queue, |worker, _| {
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
        let built = attaching.then(|| Gfx::new(init, overlay, &renderer_events));
        let gfx = match built {
            Some(Ok(gfx)) => {
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
            overlay: None,
            scale_factor,
            reader: None,
            map_events: None,
            renderer_events,
            update_scheduled: Arc::new(AtomicBool::new(false)),
            map_applied: 0,
            map_sampled: 0,
            map_published: None,
            queue: VecDeque::new(),
            waiting: Vec::new(),
            parked: VecDeque::new(),
            rendering: None,
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
        let queue = self.shared.queue.clone();
        // A caller-driven session's thread runs only when the host services
        // it, which the host may do only once it knows to draw, so the map's
        // thread tells it of the update.
        let redraw = (self.shared.driver == Driver::CallerThread).then(|| Arc::downgrade(&self.shared));
        reader.set_waker(Some(Arc::new(move || {
            if let Some(wake) = redraw.as_ref().and_then(Weak::upgrade).and_then(|shared| shared.take_redraw_wake()) {
                wake();
            }
            if !scheduled.swap(true, Ordering::SeqCst) {
                post(&queue, Worker::on_update);
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
        post(&self.shared.queue, Worker::pump);
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

    /// The map took its frame sample `sample`, and published the update.
    pub(crate) fn map_sampled(&mut self, sample: u64, outbox: &mut Outbox) {
        self.map_sampled = self.map_sampled.max(sample);
        self.pump(outbox);
    }

    /// Whether the map ran what `pending` waits for: the commands sent
    /// before it, and its frame sample.
    fn ran(&self, pending: &PendingDemand) -> bool {
        pending.sequence <= self.map_applied && pending.sample <= self.map_sampled
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

    /// Passes the renderer's events on to the map. The renderer asks for an
    /// update when something arrives for its tiles: it takes it in now, so
    /// that the work it starts runs before the next frame.
    fn forward_events(&mut self) {
        if let Some(renderer) = self.gfx.as_mut().and_then(|gfx| gfx.target.renderer_mut()) {
            renderer.receive_tiles();
        }
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
    pub(crate) fn demand(&mut self, demand: FrameDemand, sequence: u64, sample: u64, outbox: &mut Outbox) {
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
        self.queue.push_back(PendingDemand { demand, sequence, sample, serial: self.demands_received });
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
        // A sample that the map took before the session bound to it told no
        // session.
        let sampled = self.map_published.as_ref().map_or(0, |published| published.sampled());
        self.map_sampled = self.map_sampled.max(sampled);
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
            let front = self.queue.front().map(|pending| (pending.serial, self.ran(pending)));
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
                    || self.queue.iter().any(|pending| pending.serial <= behind.demands && !self.ran(pending))
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
        self.fail_if_device_lost(outbox);
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
                self.rendering = token;
                let frame = self.render(token, &update, slot);
                self.rendering = None;
                let rendered = frame.result.disposition == FrameDisposition::Rendered;
                self.publish_rendered(frame, outbox);
                // A running animation moves only when a demand samples it,
                // and this frame may have shown the update that would have
                // woken the host: the frame wakes it for the next.
                if rendered && self.map_published.as_ref().is_some_and(|published| published.animating()) {
                    self.wake_for_redraw(outbox);
                }
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
    /// frame. An offscreen target that can't begin a frame fails the image
    /// rather than waits. A host's demands neither replace nor wait for it.
    /// Ending the
    /// demands on their way ends it too, until the session can render again:
    /// a barrier demands the image again once it ran, a new target once it
    /// lands, and the next session once it attaches.
    fn serve_still(&mut self, outbox: &mut Outbox) {
        let pending = self.pending_still_image();
        // The image completed or failed meanwhile, and the next one, if one
        // waits, gets a demand of its own.
        if self.still.as_ref().is_some_and(|still| still.request != pending) {
            self.still = None;
            let result = FrameResult::unrendered(None, FrameDisposition::NoUpdate, self.latest_generation());
            self.publish(&result, outbox);
        }
        if pending == 0 {
            return;
        }
        if self.still.is_none() {
            if pending <= self.still_rendered || !self.can_render() {
                return;
            }
            self.still = Some(StillDemand { request: pending, tried: None });
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
        if !asks || tried == Some(update.generation) || self.retargeting() {
            return;
        }
        if !self.gfx.as_mut().is_some_and(|gfx| gfx.target.ready_for_frame()) {
            // An offscreen target that can't begin a frame won't by waiting:
            // the map fails the image on the error, with the demand's result
            // queued first.
            if self.gfx.as_ref().is_some_and(|gfx| gfx.target.is_offscreen()) {
                self.still = None;
                self.still_rendered = pending;
                self.publish(&FrameResult::unrendered(None, FrameDisposition::NoUpdate, update.generation), outbox);
                let error = Error::invalid_state(
                    "the offscreen target has no pixels at its size and scale factor, or more than the device's textures take",
                );
                self.forward(vec![RendererEvent::ResourceError(error)]);
            }
            return;
        }
        if self.fail_if_device_lost(outbox) {
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
        // drew the map without it, but the map fails the image on the error,
        // as it does on the image's frame's own failure on the GPU. The
        // demand ends first, so that the image fails with its result queued.
        let fails = frame.events.iter().any(|event| match event {
            RendererEvent::ResourceError(_) => true,
            RendererEvent::RenderFrameFinished { error, still_image_request_id, .. } => {
                error.is_some() && *still_image_request_id == pending
            }
            _ => false,
        });
        if fails {
            self.still = None;
            self.still_rendered = pending;
            let result = FrameResult::unrendered(None, FrameDisposition::NoUpdate, update.generation);
            self.publish_frame(frame.slot.map(|(slot, _)| (slot, None)), &result, outbox);
            return self.forward(frame.events);
        }
        match frame.result.disposition {
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
    /// Only the still image's own frame completes the image or fails it
    /// ([`disown_still`]).
    fn render(&mut self, token: Option<u64>, update: &Update, slot: Option<usize>) -> RenderedFrame {
        let ready = self.gfx.as_mut().is_some_and(|gfx| gfx.target.ready_for_frame());
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
        let handoff = gfx.target.render(&update.parameters);
        {
            let mut state = self.shared.state.lock();
            state.graphics_objects = state.graphics_objects.max(Gfx::OBJECTS);
        }
        let mut events = self.renderer_events.take();
        let dropped = gfx.target.is_presenting() && matches!(handoff, Some(Handoff::Dropped));
        let outcome = if token.is_some() {
            FrameOutcome::ForDemand
        } else if dropped {
            FrameOutcome::Dropped
        } else {
            FrameOutcome::Handed
        };
        disown_still(&mut events, outcome);
        if dropped {
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return RenderedFrame { result, events, slot: None };
        }
        let projection = gfx.target.renderer().and_then(Renderer::frame_projection);
        // The previous render's sources may hold the last references to
        // replaced data, which the renderer's workers free instead.
        let sources = update.parameters.style.get_sources();
        if let Some(previous) = self.sources.replace(Arc::clone(sources))
            && !Arc::ptr_eq(&previous, sources)
            && let Some(renderer) = gfx.target.renderer()
        {
            renderer.drop_on_workers(previous);
        }
        self.rendered_update = update.generation;
        self.rendered_generation = self.generation();
        let report = events.iter().rev().find_map(|event| match event {
            RendererEvent::RenderFrameFinished { mode, needs_repaint, repaint_at, .. } => {
                Some(FrameReport { mode: *mode, needs_repaint: *needs_repaint || repaint_at.is_some() })
            }
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
                    view: Some(update.parameters.view.map_view(self.scale_factor)),
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
        post(&self.shared.queue, Worker::still_image_changed);
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
        let released = match self.gfx.as_mut() {
            Some(gfx) if gfx.target.ring_released() => {
                let mut state = self.shared.state.lock();
                if let Some(entry) = state.ring.get_mut(slot) {
                    entry.acquired = false;
                    entry.quarantined |= kind != SyncKind::CpuComplete;
                }
                drop(state);
                return self.pump(outbox);
            }
            Some(gfx) => gfx.target.release_slot(slot, consumer),
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
        if let Some(gfx) = self.gfx.as_mut() {
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
    ) -> Result<GpuSync, Error> {
        self.set_scale_factor(scale_factor)?;
        let same_size = self.size() == size;
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        // The ring it has already: nothing changes, and the frames it holds
        // stay.
        if self.ring.as_ref() == Some(ring) && !gfx.target.ring_released() && same_size {
            // The demands that wait keep waiting for an update past the
            // replacement's resize.
            let latest = self.latest_generation();
            for (_, seen) in &mut self.waiting {
                *seen = latest;
            }
            return Ok(GpuSync::of(SyncKind::CpuComplete));
        }
        let sync = gfx.target.set_ring(ring.clone())?;
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
        // A still image that waits for a target gets its demand again, once
        // the replacement no longer counts as on its way.
        post(&self.shared.queue, Worker::still_image_changed);
        Ok(sync)
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
        self.gfx.as_mut().and_then(|gfx| gfx.target.renderer_mut()).ok_or_else(target_lost)
    }

    /// Resizes the target, and draws at `scale_factor`, after the map
    /// resized.
    pub(crate) fn resize(&mut self, size: Size, scale_factor: f64, outbox: &mut Outbox) -> Result<(), Error> {
        self.set_scale_factor(scale_factor)?;
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
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
                let resized = { gfx.target.resize_exported(physical, &held) };
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

    /// Fails the attached session when its device was lost, which it can't
    /// render on again. Returns whether it failed.
    fn fail_if_device_lost(&mut self, outbox: &mut Outbox) -> bool {
        let Some(error) = self.gfx.as_ref().and_then(|gfx| gfx.target.device().lost()) else { return false };
        log::error!(target: log_target::RENDER, "{}", error.message);
        self.fail(error, outbox);
        true
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

    /// Fails the session after a panic in its work, a bug in the library:
    /// it lets go of its renderer and target, which the panic may have left
    /// half-changed, and ends the attach and the demands on their way. A
    /// detach or release still ends the session.
    fn fail_after_panic(&mut self, message: &str, outbox: &mut Outbox) {
        let error = Error::native(format!("the render session stopped after a panic in the library: {message}"));
        log::error!(target: log_target::CRASH, "{}", error.message);
        let attach = {
            let mut state = self.shared.state.lock();
            if matches!(state.lifecycle, RenderSessionState::Attaching | RenderSessionState::Attached) {
                state.lifecycle = RenderSessionState::Failed;
                state.failure = Some(error.clone());
            }
            state.attach.take()
        };
        if let Some(attach) = attach {
            outbox.resolve(attach, Err(error));
        }
        // These run on the state that the panic left, so they may panic too.
        let _ = mln_util::panic::catch(|| {
            if let Some(token) = self.rendering.take() {
                let result = FrameResult::unrendered(Some(token), FrameDisposition::Failed, self.latest_generation());
                self.publish(&result, outbox);
            }
            self.end_demands(FrameDisposition::Failed, outbox);
            self.drop_gfx();
            self.run_queued(outbox);
        });
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
    /// ends the demands on their way. Returns, for a ring, the sync after
    /// which the host may use or destroy the textures, so a new ring keeps
    /// nothing of the old one: no texture of it stays quarantined.
    pub(crate) fn release_target(&mut self, outbox: &mut Outbox) -> Result<Option<GpuSync>, Error> {
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        let sync = gfx.target.release_surface()?;
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
        Ok(sync)
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
        let tile_cache_enabled = gfx.target.renderer().is_some_and(Renderer::tile_cache_enabled);
        let replaced = gfx.target.set_surface(surface, self.overlay.take(), physical);
        match replaced {
            // The old backend's renderer went with it: one on the new
            // backend draws what the session set.
            Ok(true) => gfx.make_renderer(&self.renderer_events, scale_factor as f32, tile_cache_enabled),
            Ok(false) => {}
            Err(_) => gfx.target.resize(physical),
        }
        let replaced = replaced.map(drop);
        self.target_changed(size, replaced.is_ok().then_some(true));
        if replaced.is_ok() {
            self.wake_for_redraw(outbox);
            // A still image that waits for a target gets its demand again.
            post(&self.shared.queue, Worker::still_image_changed);
        }
        replaced
    }

    pub(crate) fn reduce_memory_use(&mut self) -> Result<(), Error> {
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        gfx.target.with_renderer(|renderer, gpu| renderer.reduce_memory_use(gpu)).ok_or_else(target_lost)
    }

    pub(crate) fn clear_data(&mut self, outbox: &mut Outbox) -> Result<(), Error> {
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        gfx.target.with_renderer(|renderer, gpu| renderer.clear_data(gpu)).ok_or_else(target_lost)?;
        self.rendered_update = 0;
        // The renderer loads what the image needs again, once it tries.
        if let Some(still) = self.still.as_mut() {
            still.tried = None;
            post(&self.shared.queue, Worker::still_image_changed);
        }
        self.wake_for_redraw(outbox);
        Ok(())
    }

    pub(crate) fn dump_debug_logs(&mut self) -> Result<(), Error> {
        self.renderer()?.dump_debug_logs();
        Ok(())
    }

    pub(crate) fn read_texture(&mut self) -> Result<PremultipliedImage, Error> {
        let gfx = self.gfx.as_mut().ok_or_else(target_lost)?;
        if !gfx.target.is_offscreen() {
            return Err(Error::unsupported("a surface session can't read its frames back"));
        }
        if !self.has_frame {
            return Err(Error::invalid_state("the render session has no rendered frame since it attached or resized"));
        }
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
        let source =
            self.sources.as_ref().and_then(|sources| sources.get(source_id)).ok_or_else(|| {
                Error::not_found(format!("the session's last render has no source named {source_id}"))
            })?;
        let geojson = SourceData::<GeoJsonSourceType>::recover(source)
            .ok_or_else(|| Error::invalid_argument(format!("source {source_id} is not a GeoJSON source")))?;
        if !geojson.options.options.cluster {
            return Err(Error::invalid_argument(format!("GeoJSON source {source_id} does not cluster")));
        }
        let data = geojson.description.clone();
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
        if (keep || abandoned)
            && let Some(renderer) = gfx.target.renderer_mut()
        {
            renderer.lose_custom_layers();
        }
        if keep {
            // The host's graphics objects behind them may be gone already.
            std::mem::forget(gfx);
        } else if viewed {
            gfx.target.tear_down_renderer();
            self.viewed = Some(gfx.target);
        }
    }

    /// The last view of the session's frames ended: lets go of the target
    /// that a teardown kept for it, and ends a session that waited for that.
    pub(crate) fn release_viewed(&mut self) {
        if self.viewed.take().is_some() && self.finished && self.shared.driver == Driver::CoreWorker {
            self.shared.queue.stop();
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
        // A panic in the teardown, such as a custom layer's, still detaches
        // the session.
        if let Err(message) = mln_util::panic::catch(|| self.tear_down(outbox)) {
            self.fail_after_panic(&message, outbox);
        }
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
        // A target that views keep stops the queue once they end.
        if self.shared.driver == Driver::CoreWorker && self.viewed.is_none() {
            self.shared.queue.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_engine::{RenderMode, RendererEvent};
    use mln_style::error::Error;

    use super::{FrameOutcome, disown_still};

    /// The still image that a frame of the image's update, failed or not,
    /// ends once its report reaches the map.
    fn still_ended(failed: bool, outcome: FrameOutcome) -> u64 {
        let mut events = [RendererEvent::RenderFrameFinished {
            mode: RenderMode::Full,
            needs_repaint: false,
            repaint_at: None,
            placement_changed: false,
            still_image_request_id: 7,
            update_id: 0,
            stats: Arc::default(),
            error: failed.then(|| Box::new(Error::native("the device was reset"))),
        }];
        disown_still(&mut events, outcome);
        let [RendererEvent::RenderFrameFinished { still_image_request_id, .. }] = events else { unreachable!() };
        still_image_request_id
    }

    /// A frame that failed ends the still it was drawn for, whether or not
    /// a surface dropped it, so that the still fails rather than waits.
    #[test]
    fn a_failed_frame_of_the_still_ends_it_whatever_its_handoff() {
        assert_eq!(7, still_ended(true, FrameOutcome::Handed));
        assert_eq!(7, still_ended(true, FrameOutcome::Dropped));
    }

    /// A dropped frame that drew ends no still, which is drawn again, and a
    /// frame for a host's demand ends none, failed or not.
    #[test]
    fn a_dropped_frame_that_drew_or_a_frame_for_a_demand_ends_no_still() {
        assert_eq!(7, still_ended(false, FrameOutcome::Handed));
        assert_eq!(0, still_ended(false, FrameOutcome::Dropped));
        for failed in [false, true] {
            assert_eq!(0, still_ended(failed, FrameOutcome::ForDemand), "failed {failed}");
        }
    }
}
