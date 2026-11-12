//! A render session's thread: its renderer, its frame demands, and the work
//! that the handle sends it.

// The browser build renders into no borrowed ring.
#![cfg_attr(target_family = "wasm", allow(unreachable_code, unused_variables))]

use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
use mln_util::sync::MutexExt;
use mln_util::time::Instant;

use super::frame::{DemandMode, FrameDemand, FrameDisposition, FrameResult};
use super::local::{LocalTarget, Pending, PendingReads};
use super::session::{Published, RenderSessionState, SessionShared, WorkId, physical_size};
use super::target::{BorrowedRing, Device, Driver, ExportKind, ExportedRing, Surface};
use crate::completion::Resolver;
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
    Entered { previous: CURRENT.with(|current| current.borrow_mut().replace(slot.clone())) }
}

/// Runs `task` on the session whose loop is `run_loop`, after the work sent
/// before.
pub(crate) fn post(run_loop: &RunLoopHandle, task: impl FnOnce(&mut Worker, &mut Outbox) + Send + 'static) {
    run_loop.invoke(move || run(task));
}

/// Runs `task` on the session, then delivers what it sends to the host, with
/// the session unborrowed and unlocked. A session that was abandoned lets go
/// of its graphics objects first.
fn run(task: impl FnOnce(&mut Worker, &mut Outbox)) {
    let mut outbox = Outbox::default();
    if let Some(slot) = CURRENT.with(|current| current.borrow().clone()) {
        let mut slot = slot.borrow_mut();
        if let Some(worker) = slot.as_mut() {
            let shared = worker.shared.clone();
            let _gate = shared.gate.lock_unpoisoned();
            if !worker.torn_down && shared.state.lock_unpoisoned().lifecycle == RenderSessionState::Abandoned {
                worker.tear_down(&mut outbox);
            }
            task(worker, &mut outbox);
        }
    }
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
}

/// What a session renders into, before its thread builds it.
pub(crate) enum TargetInit {
    Offscreen(Option<Device>),
    Surface { surface: Surface, device: Option<Device> },
    Borrowed { ring: BorrowedRing, device: Device, producer: SyncKind },
    Exported { ring: ExportedRing, device: Device, kind: ExportKind, producer: SyncKind },
}

/// Builds the session's renderer into `slot`, on the thread that runs the
/// session's work, before any other work.
pub(crate) fn build(shared: &Arc<SessionShared>, slot: &Slot) {
    let Some(Ok(init)) = shared.state.lock_unpoisoned().init.take().map(<Box<dyn Any + Send>>::downcast::<Init>) else {
        return;
    };
    let mut outbox = Outbox::default();
    let gate = shared.gate.lock_unpoisoned();
    let worker = Worker::new(shared.clone(), *init, &mut outbox);
    *slot.borrow_mut() = Some(worker);
    drop(gate);
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
    /// How many graphics objects a `Gfx` holds: the renderer and its target.
    const OBJECTS: u32 = 2;

    fn new(init: Init) -> Result<Gfx, Error> {
        let Init { target, size, scale_factor, glyph_cache, tile_cache_enabled } = init;
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
        let mut renderer = Renderer::with_glyph_cache(backend.clone(), pixel_ratio, None, glyph_cache.clone());
        renderer.set_tile_cache_enabled(tile_cache_enabled);
        Ok(Gfx { renderer, target, backend, glyph_cache })
    }

    fn scope(&self) -> BackendScope<'_> {
        BackendScope::new(&*self.backend, ScopeType::Explicit)
    }

    /// Whether the target can take a frame now: it begins one, which a
    /// surface needs a texture for.
    fn ready_for_frame(&self) -> bool {
        let _scope = self.scope();
        self.target.frames().is_some_and(|frames| frames.begin_frame().is_some())
    }

    /// Makes the renderer again on the target's backend, which replaced the
    /// one it drew with, with what the session set on it.
    fn renew_renderer(&mut self, events: &RendererEvents) {
        self.backend = self.target.backend();
        let pixel_ratio = self.renderer.get_pixel_ratio();
        let mut renderer =
            Renderer::with_glyph_cache(self.backend.clone(), pixel_ratio, None, self.glyph_cache.clone());
        renderer.set_tile_cache_enabled(self.renderer.get_tile_cache_enabled());
        renderer.set_event_queue(Some(events.clone()));
        self.renderer = renderer;
    }
}

/// A host's frame demand on its way, and the last command sent before it.
struct PendingDemand {
    demand: FrameDemand,
    sequence: u64,
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
    /// The latest update that the session tried a still image with.
    still_attempted: u64,
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
    /// The work that waits for resizes to land, each with how many.
    behind_resizes: VecDeque<(u64, Task)>,
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
fn start_ticker(run_loop: RunLoopHandle) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
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
        let attaching = shared.state.lock_unpoisoned().lifecycle == RenderSessionState::Attaching;
        let scale_factor = init.scale_factor;
        let ring = match &init.target {
            TargetInit::Borrowed { ring, .. } => Some(ring.clone()),
            _ => None,
        };
        let built = attaching.then(|| Gfx::new(init));
        let gfx = match built {
            Some(Ok(mut gfx)) => {
                gfx.renderer.set_event_queue(Some(renderer_events.clone()));
                shared.state.lock_unpoisoned().graphics_objects = Gfx::OBJECTS;
                Some(gfx)
            }
            Some(Err(error)) => {
                let attach = {
                    let mut state = shared.state.lock_unpoisoned();
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
            queue: VecDeque::new(),
            waiting: Vec::new(),
            parked: VecDeque::new(),
            ring,
            rendered_update: 0,
            rendered_generation: 0,
            still_attempted: 0,
            has_frame: false,
            sources: None,
            torn_down: false,
            unbound: false,
            finished: false,
            resizes_landed: 0,
            behind_resizes: VecDeque::new(),
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
    pub(crate) fn bound(&mut self, sequence: u64, reader: UpdateReader, events: RendererEvents, outbox: &mut Outbox) {
        if self.torn_down {
            return;
        }
        self.map_applied = self.map_applied.max(sequence);
        let scheduled = self.update_scheduled.clone();
        let run_loop = self.shared.run_loop.clone();
        reader.set_waker(Some(Arc::new(move || {
            if !scheduled.swap(true, Ordering::SeqCst) {
                post(&run_loop, Worker::on_update);
            }
        })));
        self.reader = Some(reader);
        self.map_events = Some(events);
        let attach = {
            let mut state = self.shared.state.lock_unpoisoned();
            if state.lifecycle == RenderSessionState::Attaching {
                state.lifecycle = RenderSessionState::Attached;
            }
            state.attach.take()
        };
        if let Some(attach) = attach {
            outbox.resolve(attach, Ok(()));
        }
        // The demands that came while the session attached wait for the
        // attach to complete, which the outbox delivers once this task ends,
        // so they render in a task of their own.
        post(&self.shared.run_loop, Worker::pump);
    }

    /// The map couldn't bind the session.
    pub(crate) fn bind_failed(&mut self, error: Error, outbox: &mut Outbox) {
        let attach = {
            let mut state = self.shared.state.lock_unpoisoned();
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
        let wake = {
            let mut state = self.shared.state.lock_unpoisoned();
            let wake = state.redraw_armed && state.has_target && state.lifecycle == RenderSessionState::Attached;
            state.redraw_armed &= !wake;
            wake.then(|| self.shared.redraw_wake.clone()).flatten()
        };
        if let Some(wake) = wake {
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
        self.queue.push_back(PendingDemand { demand, sequence });
        self.pump(outbox);
    }

    /// Evaluates the demands whose commands ran and those whose update came,
    /// then a still image that the map waits for.
    fn pump(&mut self, outbox: &mut Outbox) {
        if self.torn_down {
            return;
        }
        if self.reader.is_none() {
            // A demand that came after a failed bind ends at once.
            if self.unbound {
                self.end_demands(FrameDisposition::Failed, outbox);
            }
            return;
        }
        while let Some(pending) = self.parked.pop_front() {
            if let Some(pending) = self.evaluate(pending, outbox) {
                self.parked.push_front(pending);
                return;
            }
        }
        while self.parked.is_empty() && self.queue.front().is_some_and(|pending| pending.sequence <= self.map_applied) {
            if let Some(pending) = self.queue.pop_front()
                && let Some(pending) = self.evaluate(pending, outbox)
            {
                self.parked.push_back(pending);
            }
        }
        let latest = self.latest_generation();
        // A ring replacement on its way gives the demands that wait a new
        // target, which they render to once it lands.
        let retargeting = self.shared.state.lock_unpoisoned().pending_retargets != 0;
        let (updated, waiting): (Vec<_>, Vec<_>) =
            self.waiting.drain(..).partition(|(_, seen)| !retargeting && latest > *seen);
        self.waiting = waiting;
        for (pending, _) in updated {
            if let Some(pending) = self.evaluate(pending, outbox) {
                self.parked.push_back(pending);
            }
        }
        self.render_still_image(outbox);
    }

    /// Renders the demand, or ends it, or has it wait for an update. Returns
    /// it when it waits for the ring to free a texture.
    fn evaluate(&mut self, pending: PendingDemand, outbox: &mut Outbox) -> Option<PendingDemand> {
        let token = Some(pending.demand.token);
        let latest = self.latest();
        let latest_generation = latest.as_ref().map_or(0, |update| update.generation);
        if self.gfx.is_none() {
            let failed = self.shared.state.lock_unpoisoned().lifecycle == RenderSessionState::Failed;
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
                self.render(token, &update, slot, outbox);
            }
            _ if pending.demand.mode == DemandMode::WaitForUpdate => self.waiting.push((pending, latest_generation)),
            _ => self.publish(&FrameResult::unrendered(token, FrameDisposition::NoUpdate, latest_generation), outbox),
        }
        None
    }

    /// Renders an update that asks for a still image, once for each update
    /// that reaches a target that can take the frame.
    fn render_still_image(&mut self, outbox: &mut Outbox) {
        let Some(update) = self.latest() else { return };
        let Some(gfx) = self.gfx.as_ref() else { return };
        if !update.parameters.still_image_request || update.generation <= self.still_attempted || !gfx.ready_for_frame()
        {
            return;
        }
        let slot = match self.pick_slot() {
            RingPick::Full | RingPick::Quarantined => return,
            RingPick::None => None,
            RingPick::Slot(slot) => Some(slot),
        };
        self.still_attempted = update.generation;
        self.render(None, &update, slot, outbox);
    }

    fn generation(&self) -> u64 {
        self.shared.state.lock_unpoisoned().generation
    }

    /// Renders `update` for the demand `token` into the ring's `slot`, which
    /// [`pick_slot`](Self::pick_slot) marked, or into a target without a
    /// ring, and publishes its result. A still map draws no frame until
    /// everything that it shows has loaded, and a surface without a texture
    /// to render into takes no frame.
    fn render(&mut self, token: Option<u64>, update: &Update, slot: Option<usize>, outbox: &mut Outbox) {
        let ready = self.gfx.as_ref().is_some_and(Gfx::ready_for_frame);
        let Some(gfx) = self.gfx.as_mut().filter(|_| ready) else {
            if let Some(slot) = slot {
                self.end_slot(slot, None);
            }
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return self.publish(&result, outbox);
        };
        if let Some(slot) = slot
            && gfx.target.select_slot(slot).is_err()
        {
            self.end_slot(slot, None);
            let result = FrameResult::unrendered(token, FrameDisposition::TargetNotReady, update.generation);
            return self.publish(&result, outbox);
        }
        let handoff = {
            let _scope = BackendScope::new(&*gfx.backend, ScopeType::Explicit);
            gfx.target.frames().and_then(|frames| gfx.renderer.render(&update.parameters, frames))
        };
        self.rendered_update = update.generation;
        self.rendered_generation = self.generation();
        self.sources = Some(update.parameters.sources.clone());
        let events = self.renderer_events.take();
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
                let (generation, frame_generation) = {
                    let mut state = self.shared.state.lock_unpoisoned();
                    state.frame_generation += 1;
                    (state.generation, state.frame_generation)
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
                }
            }
            None => FrameResult::unrendered(token, FrameDisposition::NoUpdate, update.generation),
        };
        if let Some(slot) = slot {
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
            self.end_slot(slot, published);
        }
        // The result goes out before the map hears of the frame, so a still
        // image completes with its frame's result queued.
        self.publish(&result, outbox);
        self.forward(events);
    }

    /// Queues a frame result for the host, and wakes it unless it was woken
    /// since its last drain.
    fn publish(&self, result: &FrameResult, outbox: &mut Outbox) {
        let wake = {
            let mut state = self.shared.state.lock_unpoisoned();
            state.results.push(*result);
            let wake = !state.frame_woken;
            state.frame_woken = true;
            wake.then(|| self.shared.frame_wake.clone()).flatten()
        };
        if let Some(wake) = wake {
            outbox.push(move || wake());
        }
    }

    /// Ends every demand on its way with `disposition`.
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
    }

    // MARK: - Borrowed rings

    /// Picks the ring's texture that the next frame renders into, and marks
    /// it as rendering: one that holds no frame, or else the one with the
    /// oldest frame that the host didn't acquire.
    fn pick_slot(&self) -> RingPick {
        let mut state = self.shared.state.lock_unpoisoned();
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
        let mut state = self.shared.state.lock_unpoisoned();
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
                let mut state = self.shared.state.lock_unpoisoned();
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
            let mut state = self.shared.state.lock_unpoisoned();
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
            return;
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
        let backend = gfx.backend.clone();
        let replaced = {
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            gfx.target.set_ring(ring.clone())
        };
        let pending = replaced?;
        let old = self.ring.replace(ring.clone());
        {
            let mut state = self.shared.state.lock_unpoisoned();
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

    /// Runs `task` once `resizes` of the session's resizes have landed, in
    /// the order of the work that waits.
    pub(crate) fn after_resizes(&mut self, resizes: u64, task: Task, outbox: &mut Outbox) {
        if resizes <= self.resizes_landed && self.behind_resizes.is_empty() {
            task(self, outbox);
        } else {
            self.behind_resizes.push_back((resizes, task));
        }
    }

    fn resize_landed(&mut self, outbox: &mut Outbox) {
        self.resizes_landed += 1;
        while self.behind_resizes.front().is_some_and(|(resizes, _)| *resizes <= self.resizes_landed) {
            if let Some((_, task)) = self.behind_resizes.pop_front() {
                task(self, outbox);
            }
        }
    }

    /// The target's size in logical pixels.
    pub(crate) fn size(&self) -> Size {
        self.shared.state.lock_unpoisoned().size
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
                    let mut state = self.shared.state.lock_unpoisoned();
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
                        let mut state = self.shared.state.lock_unpoisoned();
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
            let mut state = self.shared.state.lock_unpoisoned();
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
        let mut state = self.shared.state.lock_unpoisoned();
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
        let backend = gfx.backend.clone();
        let pending = {
            let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
            gfx.target.release_surface()
        };
        self.has_frame = false;
        self.ring = None;
        {
            let mut state = self.shared.state.lock_unpoisoned();
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
        let backend = gfx.backend.clone();
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
            .as_any()
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
        let attach = self.shared.state.lock_unpoisoned().attach.take();
        if let Some(attach) = attach {
            outbox.resolve(attach, Err(Error::target_lost("the render session ended before it attached")));
        }
    }

    /// Lets go of the renderer and its target: frees them, unless the
    /// session keeps them until the process exits. While a view is open on
    /// one of the ring's frames, the target and its backend stay until the
    /// last view ends.
    fn drop_gfx(&mut self) {
        let Some(gfx) = self.gfx.take() else { return };
        let (keep, viewed) = {
            let mut state = self.shared.state.lock_unpoisoned();
            state.graphics_objects = 0;
            let viewed = state.views != 0 && !state.keep_graphics;
            state.viewed_teardown |= viewed;
            (state.keep_graphics, viewed)
        };
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
        let map = self.shared.state.lock_unpoisoned().map.clone();
        if let Some(map) = map {
            map.unbind_session(self.shared.id);
        }
        self.finish(outbox);
    }

    /// Ends the session: it completes the detach and the releases, lets go
    /// of the map, and stops the session's thread.
    pub(crate) fn finish(&mut self, outbox: &mut Outbox) {
        let (waiters, map) = {
            let mut state = self.shared.state.lock_unpoisoned();
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
