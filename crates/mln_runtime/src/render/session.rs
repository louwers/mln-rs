//! The handle of a render session, and the state that its thread shares
//! with the handle.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;

use mln_actor::{RunLoop, RunLoopHandle};
use mln_map::LogicalExtent;
use mln_renderer::query::{RenderedQueryOptions, SourceQueryOptions};
use mln_util::error::Error;
use mln_util::feature::Feature;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use parking_lot::Mutex;

use super::driver::DriverWork;
use super::frame::{FrameDemand, FrameResult};
use super::local::Pending;
use super::query::RenderedQueryGeometry;
use super::target::{
    AttachOptions, BorrowedRing, ConsumerSync, Device, DeviceLease, Driver, ExportKind, FrameTexture, GpuSync,
    RenderTarget, Surface, SyncKind, SyncKinds, Wake,
};
use super::worker::{self, Init, TargetInit, Worker};
use crate::completion::{Completion, Home, Resolver};
use crate::map::{Applied, Map};
use crate::runtime::Outbox;

/// The ID of a render session, unique in the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SessionId(u64);

/// Where a render session is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderSessionState {
    /// The session is starting: it builds its renderer and binds to its map.
    Attaching,
    Attached,
    /// A detach or release is on its way.
    Detaching,
    /// The session let go of its map and its graphics objects.
    Detached,
    /// The session couldn't build its renderer. Its snapshot says why.
    Failed,
    Abandoned,
}

/// The state of a render session at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSessionSnapshot {
    pub state: RenderSessionState,
    /// The target's logical size, and the map's pixel ratio.
    pub extent: LogicalExtent,
    /// Grows each time the target changes size or is replaced.
    pub generation: u64,
    /// Counts the frames that the session rendered.
    pub frame_generation: u64,
    /// Why the session failed.
    pub failure: Option<Error>,
    /// Whether the session is attached and has a target to render to. It is
    /// false from a [`release_target`](RenderSession::release_target) until
    /// a [`set_surface`](RenderSession::set_surface) gives it a new one.
    pub has_target: bool,
    /// How many of a borrowed ring's frames the host holds.
    pub acquired_frames: u32,
}

/// What a render session can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderSessionCapabilities {
    pub driver: Driver,
    /// How many textures the session's texture ring has.
    pub texture_ring_depth: u32,
    /// Whether the host can take the frames it renders.
    pub frame_acquisition: bool,
    /// Whether [`RenderSession::read_texture`] can read its pixels.
    pub readback: bool,
    /// Whether it shows frames on a surface.
    pub presentation: bool,
    /// The kind of sync that a borrowed ring's frames carry.
    pub producer_sync_kind: Option<SyncKind>,
    /// The kinds of sync that [`AcquiredFrame::release`] takes.
    pub consumer_sync_kinds: SyncKinds,
    /// How an exported ring shares its images, for an exported ring.
    pub export_kind: Option<ExportKind>,
}

/// A frame of a borrowed ring that the session published, and what the
/// host waits for before it reads it.
#[derive(Clone, Debug)]
pub(crate) struct Published {
    pub(crate) result: FrameResult,
    pub(crate) sync: GpuSync,
    pub(crate) texture: Option<FrameTexture>,
}

/// One texture of a borrowed ring.
#[derive(Clone, Debug, Default)]
pub(crate) struct RingSlot {
    /// The newest frame rendered into the texture, which the host hasn't
    /// acquired.
    pub(crate) frame: Option<Published>,
    pub(crate) acquired: bool,
    pub(crate) rendering: bool,
    /// The host's GPU may still read the texture, so the session renders
    /// into it no more, even when a new ring has it again, until a release
    /// of the ring.
    pub(crate) quarantined: bool,
    /// The host holds an exported ring's image from before a resize, which
    /// gave the slot a new one that never reached the host: giving the old
    /// one up quarantines nothing.
    pub(crate) retired: bool,
}

impl RingSlot {
    pub(crate) fn busy(&self) -> bool {
        self.acquired || self.rendering || self.quarantined
    }
}

/// Work sent to a session's thread, whose completion an abandon can fail at
/// once instead of waiting for the thread to run it.
pub(crate) trait PendingWork: Any + Send {
    fn fail(self: Box<Self>, error: Error);
}

impl<T: Send + 'static> PendingWork for Resolver<T> {
    fn fail(self: Box<Self>, error: Error) {
        (*self).resolve(Err(error));
    }
}

/// The ID of a piece of work that waits for a session's thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WorkId(u64);

/// The state that a session's handle and its thread share.
pub(crate) struct SessionState {
    pub(crate) lifecycle: RenderSessionState,
    pub(crate) released: bool,
    pub(crate) size: Size,
    /// The scale factor that the target has, as `size` does.
    pub(crate) scale_factor: f64,
    /// The scale factor of the last resize sent, which a later resize that
    /// names none keeps.
    pub(crate) sent_scale_factor: f64,
    pub(crate) generation: u64,
    pub(crate) frame_generation: u64,
    pub(crate) failure: Option<Error>,
    /// Whether the session has a target to render to: false from a release
    /// of its target until it gets a new one.
    pub(crate) has_target: bool,
    /// The frame results that wait for a drain.
    pub(crate) results: Vec<FrameResult>,
    /// Whether the frame wake ran since the last drain.
    pub(crate) frame_woken: bool,
    /// Whether the redraw wake may run: from the last demand until it runs.
    pub(crate) redraw_armed: bool,
    /// The attach's completion, until the map binds the session.
    pub(crate) attach: Option<Resolver<()>>,
    /// The completions of the detaches on their way.
    pub(crate) detach_waiters: Vec<Resolver<()>>,
    /// The completions of the releases on their way.
    pub(crate) release_waiters: Vec<Resolver<()>>,
    /// The completions of the work that waits for the session's thread.
    pending: BTreeMap<WorkId, Box<dyn PendingWork>>,
    next_work: u64,
    /// The session's map, until the session lets go of it.
    pub(crate) map: Option<Map>,
    /// What the session's thread builds the renderer from, until it does:
    /// an [`Init`]. It is boxed as `Any` so that the state names no
    /// graphics types, whose auto traits are deep to prove for every type
    /// that holds a map handle.
    pub(crate) init: Option<Box<dyn Any + Send>>,
    /// Counts the resizes sent, which run on the map's thread before they
    /// reach the session's.
    pub(crate) resizes_sent: u64,
    /// How many graphics objects the session's thread holds.
    pub(crate) graphics_objects: u32,
    /// Whether the session's thread keeps its graphics objects until the
    /// process exits, instead of freeing them, when the session ends.
    pub(crate) keep_graphics: bool,
    /// Whether an abandon keeps the graphics objects on any thread, as
    /// `Device::abandon_keeps_graphics` says of the session's device.
    pub(crate) abandon_keeps_graphics: bool,
    /// The slots of a borrowed ring, and none for another target.
    pub(crate) ring: Vec<RingSlot>,
    /// How many of the ring's frames the host holds.
    pub(crate) acquired_frames: u32,
    /// The ring replacements and releases on their way: no frame is
    /// acquired until they ran, since the frames published before one name
    /// textures it replaces or lets go of.
    pub(crate) pending_retargets: u32,
    /// How many views are open on the session's acquired frames.
    pub(crate) views: u32,
    /// Whether the session's thread keeps graphics objects that the open
    /// views name, which it lets go of once the last view ends.
    pub(crate) viewed_teardown: bool,
}

impl SessionState {
    /// Keeps `resolver` until the session's thread takes it.
    fn register<T: Send + 'static>(&mut self, resolver: Resolver<T>) -> WorkId {
        self.next_work += 1;
        let id = WorkId(self.next_work);
        self.pending.insert(id, Box::new(resolver));
        id
    }

    /// What an end of the session completes, once the state is unlocked.
    pub(crate) fn take_waiters(&mut self) -> Waiters {
        Waiters {
            attach: self.attach.take(),
            pending: std::mem::take(&mut self.pending).into_values().collect(),
            detaches: std::mem::take(&mut self.detach_waiters),
            releases: std::mem::take(&mut self.release_waiters),
        }
    }
}

/// The completions that wait for a session when it ends.
pub(crate) struct Waiters {
    attach: Option<Resolver<()>>,
    pending: Vec<Box<dyn PendingWork>>,
    detaches: Vec<Resolver<()>>,
    releases: Vec<Resolver<()>>,
}

impl Waiters {
    /// Completes the releases, and the detaches with `detached`. The attach
    /// and the work that never ran fail with `lost`.
    pub(crate) fn settle(self, detached: &Result<(), Error>, lost: impl Fn() -> Error) {
        if let Some(attach) = self.attach {
            attach.resolve(Err(lost()));
        }
        for work in self.pending {
            work.fail(lost());
        }
        for waiter in self.detaches {
            waiter.resolve(detached.clone());
        }
        for waiter in self.releases {
            waiter.resolve(Ok(()));
        }
    }
}

/// Who abandons a session.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Abandoner {
    /// The host, through the handle.
    Host,
    /// The library, for a session that the host released, or stopped
    /// servicing.
    Library,
}

pub(crate) struct SessionShared {
    pub(crate) id: SessionId,
    pub(crate) driver: Driver,
    pub(crate) run_loop: RunLoopHandle,
    /// The thread that runs the session's work, once it is known: a core
    /// worker's from the attach, and a caller thread's from when the host
    /// takes the driver work.
    pub(crate) thread: Arc<OnceLock<ThreadId>>,
    /// The thread of the map's runtime, which runs the map's side of the
    /// session's work.
    runtime_thread: ThreadId,
    pub(crate) state: Mutex<SessionState>,
    /// Held while the session's thread runs a task, so an abandon waits for
    /// the graphics work in progress.
    pub(crate) gate: Mutex<()>,
    /// The session's lease on the host's device, which an abandon revokes,
    /// since graphics objects that it keeps hold the device.
    lease: DeviceLease,
    /// Held while [`RenderSession::send_then`] sends a command and attaches
    /// its callback, and while an attach hands its completion out, which the
    /// session's thread waits for before it delivers results.
    sending: Mutex<()>,
    /// Whether the session's thread is inside a driver call: a core worker
    /// running a task and delivering what it sent the host, or a service of
    /// a caller-driven session.
    pub(crate) in_call: AtomicBool,
    pub(crate) frame_wake: Option<Wake>,
    pub(crate) redraw_wake: Option<Wake>,
    capabilities: RenderSessionCapabilities,
    driver_work_taken: AtomicBool,
    /// Whether the driver-work wake may run: from the start of the last
    /// service until it runs.
    pub(crate) driver_armed: Arc<AtomicBool>,
    /// The first of the map's commands that the session's work waits for,
    /// or `u64::MAX` for none: the map tells the session's thread when it
    /// ran a command only from that one on, so a command that no work waits
    /// for wakes no caller's driver.
    pub(crate) awaited_command: Arc<AtomicU64>,
    /// In the browser, whether a caller-driven session waits for the device
    /// that it asked the browser's WebGPU for: its services build nothing
    /// until the device is there, and the driver-work wake runs then.
    pub(crate) preparing: AtomicBool,
}

impl SessionShared {
    /// A completion that the session's thread resolves.
    fn completion<T: Send + 'static>(&self) -> (Completion<T>, Resolver<T>) {
        Completion::at(Home::Later(self.thread.clone(), self.runtime_thread))
    }

    /// The completion of the work `id`, unless an end of the session
    /// completed it first.
    pub(crate) fn take_work<T: Send + 'static>(&self, id: WorkId) -> Option<Resolver<T>> {
        let work: Box<dyn Any + Send> = self.state.lock().pending.remove(&id)?;
        work.downcast::<Resolver<T>>().ok().map(|resolver| *resolver)
    }

    pub(crate) fn is_released(&self) -> bool {
        self.state.lock().released
    }

    /// The redraw wake that tells the host the map has something new to
    /// draw, unless the host was told since its last demand, or the session
    /// has no target. The host isn't told again until its next demand.
    pub(crate) fn take_redraw_wake(&self) -> Option<Wake> {
        let mut state = self.state.lock();
        let wake = state.redraw_armed && state.has_target && state.lifecycle == RenderSessionState::Attached;
        state.redraw_armed &= !wake;
        wake.then(|| self.redraw_wake.clone()).flatten()
    }

    /// Starts a detach on the session's thread, after the work sent before.
    /// The state is locked, and the session usable or failed.
    fn start_detach(&self, state: &mut SessionState) {
        state.lifecycle = RenderSessionState::Detaching;
        worker::post(&self.run_loop, Worker::detach);
    }

    /// Releases the session without waiting: it detaches, unless it is
    /// detached or abandoned already, and every call on it fails after. A
    /// caller-driven session that isn't attached, or whose map was released,
    /// is abandoned instead, so the release doesn't wait for the host to
    /// service it.
    fn dispose(&self, done: Option<Resolver<()>>) -> Result<(), Error> {
        with_unlocked_wakes(|| self.dispose_locked(done))
    }

    /// Releases the session without waiting and without graphics calls past
    /// the one in progress: a session that hasn't ended is abandoned, and
    /// keeps its graphics objects until the process exits.
    fn dispose_keeping_graphics(&self) -> Result<(), Error> {
        with_unlocked_wakes(|| {
            {
                let mut state = self.state.lock();
                if state.released {
                    return Err(released());
                }
                state.released = true;
                state.keep_graphics = true;
                if matches!(state.lifecycle, RenderSessionState::Detached | RenderSessionState::Abandoned) {
                    return Ok(());
                }
            }
            if self.abandon_locked(Abandoner::Library, true).is_err() {
                // Only the session's own work could hold the gate here: the
                // session detaches after it, keeping its graphics objects.
                let mut state = self.state.lock();
                if !matches!(
                    state.lifecycle,
                    RenderSessionState::Detaching | RenderSessionState::Detached | RenderSessionState::Abandoned
                ) {
                    self.start_detach(&mut state);
                }
            }
            Ok(())
        })
    }

    fn dispose_locked(&self, done: Option<Resolver<()>>) -> Result<(), Error> {
        let mut state = self.state.lock();
        if state.released {
            return Err(released());
        }
        state.released = true;
        // A released map waits for no service of its session's detach.
        let map_released = state.map.as_ref().is_some_and(Map::is_released);
        match state.lifecycle {
            RenderSessionState::Detached | RenderSessionState::Abandoned => {
                drop(state);
                if let Some(done) = done {
                    done.resolve(Ok(()));
                }
                return Ok(());
            }
            RenderSessionState::Detaching => {}
            RenderSessionState::Attaching | RenderSessionState::Failed if self.driver == Driver::CallerThread => {
                state.release_waiters.extend(done);
                drop(state);
                self.abandon_or_detach();
                return Ok(());
            }
            RenderSessionState::Attached if self.driver == Driver::CallerThread && map_released => {
                state.release_waiters.extend(done);
                drop(state);
                self.abandon_or_detach();
                return Ok(());
            }
            RenderSessionState::Attaching | RenderSessionState::Attached | RenderSessionState::Failed => {
                self.start_detach(&mut state);
            }
        }
        state.release_waiters.extend(done);
        Ok(())
    }

    /// Abandons the released session, or detaches it when its own work
    /// holds the gate, which only it could here.
    fn abandon_or_detach(&self) {
        if self.abandon(Abandoner::Library).is_err() {
            let mut state = self.state.lock();
            if state.lifecycle != RenderSessionState::Abandoned {
                self.start_detach(&mut state);
            }
        }
    }

    /// Stops the session at once, as [`RenderSession::abandon`] documents,
    /// and returns how many graphics objects it keeps. The library may
    /// abandon a released session too.
    pub(crate) fn abandon(&self, by: Abandoner) -> Result<u32, Error> {
        with_unlocked_wakes(|| self.abandon_locked(by, false))
    }

    /// Waits for a [`RenderSession::send_then`] in progress to attach its
    /// callback. The session's thread calls it before it delivers what it
    /// sent the host, so that a command's result reaches the callback there,
    /// inside the driver call.
    pub(crate) fn wait_for_senders(&self) {
        drop(self.sending.lock());
    }

    /// Takes the sending lock, which the session's thread waits for before
    /// it delivers results, unless this thread holds it already: a send
    /// inside another, such as from a completion that a release settles at
    /// once on this thread, is delivered after the outer one too.
    fn hold_sending(&self) -> Option<Sending<'_>> {
        if SENDING.with(|sending| sending.borrow().contains(&self.id)) {
            return None;
        }
        let guard = self.sending.lock();
        SENDING.with(|sending| sending.borrow_mut().push(self.id));
        Some(Sending { id: self.id, _guard: guard })
    }

    /// Marks the session's thread as inside a driver call until the guard
    /// drops.
    pub(crate) fn enter_call(&self) -> InCall<'_> {
        InCall { shared: self, previous: self.in_call.swap(true, Ordering::SeqCst) }
    }

    /// Abandons the session, which keeps its graphics objects until the
    /// process exits if `keep_graphics`, and returns how many it keeps.
    ///
    /// A driver call can't be waited for on its own thread, where the
    /// session's callbacks run inside it, and a host's service on another
    /// thread is the host's to end, so either is busy. A core worker's call
    /// on another thread is waited for. A caller-driven session that is
    /// abandoned off its thread keeps its graphics objects, which only its
    /// thread may free: once this returns, the host may destroy what they
    /// were made on.
    fn abandon_locked(&self, by: Abandoner, keep_graphics: bool) -> Result<u32, Error> {
        let on_driver = self.thread.get() == Some(&std::thread::current().id());
        if self.in_call.load(Ordering::SeqCst) {
            if on_driver {
                return Err(Error::busy("the render session's driver call is running on this thread"));
            }
            if self.driver == Driver::CallerThread && by == Abandoner::Host {
                return Err(Error::busy("the render session's driver work is running on another thread"));
            }
        }
        let off_thread = self.driver == Driver::CallerThread && !on_driver;
        let gate = if on_driver {
            self.gate.try_lock().ok_or_else(|| Error::busy("the render session is rendering on this thread"))?
        } else {
            self.gate.lock()
        };
        let (waiters, map, kept) = {
            let mut state = self.state.lock();
            if by == Abandoner::Host && state.released {
                return Err(released());
            }
            match state.lifecycle {
                RenderSessionState::Attaching
                | RenderSessionState::Attached
                | RenderSessionState::Detaching
                | RenderSessionState::Failed => {}
                RenderSessionState::Detached | RenderSessionState::Abandoned => {
                    return Err(Error::invalid_state("the render session was detached or abandoned"));
                }
            }
            // The views keep the frames' sync objects in place, which the
            // abandon would let go of. A library's abandon, of a session
            // that the host released, keeps them until the views end.
            if by == Abandoner::Host && state.views != 0 {
                return Err(Error::busy("a view is open on one of the render session's acquired frames"));
            }
            state.lifecycle = RenderSessionState::Abandoned;
            state.keep_graphics |= keep_graphics || off_thread || state.abandon_keeps_graphics;
            let kept = if state.keep_graphics { state.graphics_objects } else { 0 };
            (state.take_waiters(), state.map.take(), kept)
        };
        match &map {
            // The map unbinds the session before another can attach.
            Some(map) => {
                map.unbind_session_then(self.id, self.run_loop.clone(), Worker::finish);
                map.release_session_claim(self.id);
            }
            None => worker::post(&self.run_loop, Worker::finish),
        }
        // A core worker lets go of its graphics objects at its next task,
        // which runs before this returns: the host may destroy what they were
        // made on once it does.
        // In the browser the session's loop runs on this thread, which runs
        // that task here instead, unless it is inside the session's work.
        #[cfg(target_family = "wasm")]
        let run_now = self.driver == Driver::CoreWorker
            && !self.in_call.load(Ordering::SeqCst)
            && !SENDING.with(|sending| sending.borrow().contains(&self.id));
        let torn_down = (self.driver == Driver::CoreWorker && !on_driver).then(|| {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<()>(1);
            worker::post(&self.run_loop, move |_, _| {
                let _ = sender.send(());
            });
            receiver
        });
        drop(gate);
        if let Some(receiver) = torn_down {
            // A worker that ended drops the task, and the sender with it.
            let _ = receiver.recv();
        }
        #[cfg(target_family = "wasm")]
        if run_now {
            worker::post(&self.run_loop, |_, _| {});
            crate::web::run_session_now(self.id);
        }
        // Off the session's thread, the session makes no more graphics
        // calls: a core worker tore down above, and a caller-driven session
        // keeps its graphics objects. On it, the teardown that follows
        // still needs the device.
        if !on_driver {
            self.lease.revoke();
        }
        if kept != 0 {
            let reason = if keep_graphics {
                "it was abandoned without graphics calls"
            } else if off_thread {
                "only the thread that services it may free them"
            } else {
                "only a detach frees OpenGL and WebGPU objects"
            };
            Log::warning(
                Event::Render,
                format!("An abandoned render session keeps {kept} graphics objects until the process exits: {reason}"),
            );
        }
        let lost = || Error::target_lost("the render session was abandoned");
        waiters.settle(&Err(lost()), lost);
        drop(map);
        Ok(kept)
    }
}

/// Marks a session's thread as inside a driver call, as
/// [`SessionShared::enter_call`] does.
pub(crate) struct InCall<'a> {
    shared: &'a SessionShared,
    previous: bool,
}

impl Drop for InCall<'_> {
    fn drop(&mut self) {
        self.shared.in_call.store(self.previous, Ordering::SeqCst);
    }
}

fn released() -> Error {
    Error::invalid_state("the render session was released")
}

/// Fails unless the session takes work: it is attaching or attached, and
/// not released.
fn check_usable(state: &SessionState) -> Result<(), Error> {
    if state.released {
        return Err(released());
    }
    match state.lifecycle {
        RenderSessionState::Attaching | RenderSessionState::Attached => Ok(()),
        RenderSessionState::Failed => {
            Err(state.failure.clone().unwrap_or_else(|| Error::native("the render session failed")))
        }
        RenderSessionState::Detaching | RenderSessionState::Detached | RenderSessionState::Abandoned => {
            Err(Error::invalid_state("the render session is not attached: it was detached or abandoned"))
        }
    }
}

thread_local! {
    /// The driver wakes that this thread's posts asked for during a handle
    /// call, which may hold the session's locks.
    static DEFERRED_WAKES: RefCell<Option<Vec<Wake>>> = const { RefCell::new(None) };
    /// The sessions whose sending lock this thread holds.
    static SENDING: RefCell<Vec<SessionId>> = const { RefCell::new(Vec::new()) };
}

/// A session's sending lock, which [`SessionShared::hold_sending`] took.
struct Sending<'a> {
    id: SessionId,
    _guard: parking_lot::MutexGuard<'a, ()>,
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        SENDING.with(|sending| sending.borrow_mut().retain(|id| *id != self.id));
    }
}

/// Runs `call`, which may post to a session's loop with the session's locks
/// held, then the driver wakes that its posts asked for, with nothing locked.
fn with_unlocked_wakes<R>(call: impl FnOnce() -> R) -> R {
    let outer = DEFERRED_WAKES.with(|deferred| deferred.borrow_mut().replace(Vec::new()));
    let result = call();
    let wakes = DEFERRED_WAKES.with(|deferred| {
        let mut deferred = deferred.borrow_mut();
        let wakes = deferred.take().unwrap_or_default();
        match outer {
            // The outer call runs them.
            Some(mut outer) => {
                outer.extend(wakes);
                *deferred = Some(outer);
                Vec::new()
            }
            None => wakes,
        }
    });
    for wake in wakes {
        wake();
    }
    result
}

/// Runs `wake` now, or once the handle call on this thread returns.
fn wake_unlocked(wake: &Wake) {
    let deferred = DEFERRED_WAKES.with(|deferred| match deferred.borrow_mut().as_mut() {
        Some(wakes) => {
            wakes.push(wake.clone());
            true
        }
        None => false,
    });
    if !deferred {
        wake();
    }
}

/// Sizes in pixels a target of `size` logical pixels at `pixel_ratio`.
pub(crate) fn physical_size(size: Size, pixel_ratio: f32) -> Size {
    Size::new((size.width as f32 * pixel_ratio) as u32, (size.height as f32 * pixel_ratio) as u32)
}

/// A render session: a renderer that draws a map into a target, on a thread
/// of its own or on the host's.
///
/// The host demands frames, which finish with [`FrameResult`]s that
/// [`drain_frame_results`](Self::drain_frame_results) takes. A demand shows
/// the map's commands sent before it, except the changes that an open
/// command group holds back. The session's work runs on its thread in the
/// order it was sent, and reports through a [`Completion`]. `Clone` is
/// another handle to the same session, and any thread may use one.
///
/// A session that attached with [`Driver::CallerThread`] runs its work only
/// when the host services its [`DriverWork`], which
/// [`take_driver_work`](Self::take_driver_work) hands out.
///
/// The session lives until [`detach`](Self::detach),
/// [`release`](Self::release), [`dispose`](Self::dispose) or
/// [`abandon`](Self::abandon), or until its last handle drops, and it holds
/// its map until then. A call that sends work to a session that no longer
/// takes it fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind), or
/// with the failure of a session that failed.
#[derive(Clone)]
pub struct RenderSession {
    handle: Arc<SessionHandle>,
}

/// Disposes of the session when the last handle drops.
struct SessionHandle {
    shared: Arc<SessionShared>,
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let _ = self.shared.dispose(None);
    }
}

impl std::fmt::Debug for RenderSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderSession").field("id", &self.shared().id).finish()
    }
}

impl PartialEq for RenderSession {
    fn eq(&self, other: &RenderSession) -> bool {
        Arc::ptr_eq(&self.handle, &other.handle)
    }
}

impl Eq for RenderSession {}

/// Attaches a session to `map`, as [`Map::attach`] documents, and runs `then`
/// with the session and the attach's completion before the session's thread
/// may deliver that, as [`Map::attach_then`] documents.
pub(crate) fn attach<R>(
    map: &Map,
    target: RenderTarget,
    options: AttachOptions,
    then: impl FnOnce(&RenderSession, Completion<()>) -> R,
) -> Result<R, Error> {
    // The wakes that the attach runs, such as a caller-driven session's
    // driver-work wake, wait for the release of the session's sending lock,
    // so a host's wake that services the session can't wait for it.
    with_unlocked_wakes(|| attach_unlocked(map, target, options, then))
}

fn attach_unlocked<R>(
    map: &Map,
    target: RenderTarget,
    options: AttachOptions,
    then: impl FnOnce(&RenderSession, Completion<()>) -> R,
) -> Result<R, Error> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let mut ring = None;
    let mut export_kind = None;
    let (size, target) = match target {
        RenderTarget::Offscreen { size, device } => (size, TargetInit::Offscreen(device)),
        RenderTarget::Surface { size, surface, device } => {
            if device.is_none() && !surface.needs_no_device() {
                return Err(Error::invalid_argument("a surface that the host made needs the host's device"));
            }
            surface.check_area()?;
            surface.check_pixel_format(device.as_ref())?;
            (size, TargetInit::Surface { surface, device })
        }
        RenderTarget::Borrowed { size, ring: borrowed, device } => {
            let (producer, consumer) = device.borrowed_sync_kinds(&borrowed)?;
            let picked = options.producer_sync_kinds.pick(producer).ok_or_else(|| {
                Error::unsupported("render target supports none of the requested producer sync kinds")
            })?;
            if borrowed.depth() == 0 || borrowed.physical_size().is_empty() {
                return Err(Error::invalid_argument("a borrowed ring needs textures of a size"));
            }
            ring = Some((borrowed.depth(), picked, consumer));
            (size, TargetInit::Borrowed { ring: borrowed, device, producer: picked })
        }
        RenderTarget::Exported { size, ring: exported, device } => {
            if exported.depth == 0 {
                return Err(Error::invalid_argument("an exported ring needs at least one image"));
            }
            if exported.kinds.is_empty() {
                return Err(Error::invalid_argument("an exported ring needs a kind of image to share"));
            }
            if size.is_empty() {
                return Err(Error::invalid_argument("the target's size must not be empty"));
            }
            let device = match device {
                Some(device) => device,
                None => Device::for_exported_ring(&exported)?,
            };
            let (kind, producer, consumer) = device.export_plan(&exported)?;
            let picked = options.producer_sync_kinds.pick(producer).ok_or_else(|| {
                Error::unsupported("render target supports none of the requested producer sync kinds")
            })?;
            ring = Some((exported.depth, picked, consumer));
            export_kind = Some(kind);
            (size, TargetInit::Exported { ring: exported, device, kind, producer: picked })
        }
    };
    if size.is_empty() {
        return Err(Error::invalid_argument("the target's size must not be empty"));
    }
    let (target, lease) = target.lease();
    let presentation = matches!(target, TargetInit::Surface { .. });
    let run_loop = RunLoopHandle::new();
    let driver_armed = Arc::new(AtomicBool::new(true));
    if options.driver == Driver::CallerThread
        && let Some(wake) = options.driver_work_wake
    {
        let armed = driver_armed.clone();
        run_loop.set_waker(Some(Arc::new(move || {
            if armed.swap(false, Ordering::SeqCst) {
                wake_unlocked(&wake);
            }
        })));
    }
    let id = SessionId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let scale_factor = map.scale_factor();
    let shared = Arc::new(SessionShared {
        id,
        driver: options.driver,
        run_loop: run_loop.clone(),
        thread: Arc::default(),
        runtime_thread: map.runtime_thread(),
        state: Mutex::new(SessionState {
            lifecycle: RenderSessionState::Attaching,
            released: false,
            size,
            scale_factor,
            sent_scale_factor: scale_factor,
            generation: 1,
            frame_generation: 0,
            failure: None,
            has_target: true,
            results: Vec::new(),
            frame_woken: false,
            redraw_armed: true,
            attach: None,
            detach_waiters: Vec::new(),
            release_waiters: Vec::new(),
            pending: BTreeMap::new(),
            next_work: 0,
            map: Some(map.clone()),
            init: Some(Box::new(Init {
                target,
                size,
                scale_factor,
                glyph_cache: map.glyph_cache(),
                tile_cache_enabled: map.tile_cache_enabled(),
                failure: None,
            })),
            resizes_sent: 0,
            graphics_objects: 0,
            keep_graphics: false,
            abandon_keeps_graphics: false,
            ring: vec![RingSlot::default(); ring.map_or(0, |(depth, _, _)| depth)],
            acquired_frames: 0,
            pending_retargets: 0,
            views: 0,
            viewed_teardown: false,
        }),
        gate: Mutex::new(()),
        lease,
        frame_wake: options.frame_wake,
        redraw_wake: options.redraw_wake,
        capabilities: RenderSessionCapabilities {
            driver: options.driver,
            texture_ring_depth: ring.map_or(1, |(depth, _, _)| depth as u32),
            frame_acquisition: ring.is_some(),
            readback: !presentation && ring.is_none(),
            presentation,
            producer_sync_kind: ring.map(|(_, producer, _)| producer),
            consumer_sync_kinds: ring.map_or(SyncKinds::NONE, |(_, _, consumer)| consumer),
            export_kind,
        },
        driver_work_taken: AtomicBool::new(false),
        driver_armed,
        awaited_command: Arc::new(AtomicU64::new(u64::MAX)),
        preparing: AtomicBool::new(false),
        in_call: AtomicBool::new(false),
        sending: Mutex::new(()),
    });
    let (completion, resolver) = shared.completion();
    shared.state.lock().attach = Some(resolver);
    // Held until `then` returns: the session's thread delivers the attach's
    // result after, inside its driver call.
    let sending = shared.clone();
    let _sending = sending.hold_sending();
    map.claim_session(id, Arc::downgrade(&shared))?;
    if options.driver == Driver::CoreWorker {
        match start_thread(shared.clone()) {
            Ok(thread) => {
                let _ = shared.thread.set(thread);
            }
            Err(error) => {
                map.release_session_claim(id);
                return Err(error);
            }
        }
    }
    let session = RenderSession { handle: Arc::new(SessionHandle { shared }) };
    if let Err(error) = map.bind_session(id, run_loop, session.shared().awaited_command.clone(), size) {
        // The map was released meanwhile: the session lets go of it.
        drop(session);
        return Err(error);
    }
    if options.driver == Driver::CallerThread {
        #[cfg(target_family = "wasm")]
        prepare_caller_driven(session.shared());
        // The first service builds the renderer, which is work that is ready
        // now: this wakes the host, unless the map's first update, posted
        // from the runtime's thread, woke it already.
        session.shared().run_loop.invoke(|| {});
    }
    Ok(then(&session, completion))
}

/// Starts a core worker's thread, which builds the session's renderer, then
/// runs the session's loop until the session ends.
#[cfg(not(target_family = "wasm"))]
fn start_thread(shared: Arc<SessionShared>) -> Result<ThreadId, Error> {
    let thread = mln_util::thread::builder("RenderSession")
        .spawn(move || {
            let Some(run_loop) = RunLoop::adopt(&shared.run_loop) else { return };
            let _current = run_loop.enter();
            let slot = worker::Slot::default();
            let entered = worker::enter(&slot);
            worker::build(&shared, &slot);
            // The worker holds the session from here on.
            drop(shared);
            run_loop.run();
            let worker = slot.borrow_mut().take();
            drop(worker);
            drop(entered);
        })
        .map_err(|error| Error::native(format!("can't start the render session's thread: {error}")))?;
    let thread_id = thread.thread().id();
    // The thread ends by itself once the session ends, so nothing joins it.
    drop(thread);
    Ok(thread_id)
}

/// Runs a core worker on this thread, the browser's one, in the instance's
/// pump: it builds the session's renderer once its device is ready, which
/// may take the browser's WebGPU a while, then runs the session's loop until
/// the session ends.
#[cfg(target_family = "wasm")]
#[expect(clippy::unnecessary_wraps, reason = "the threads of other builds can fail to start")]
fn start_thread(shared: Arc<SessionShared>) -> Result<ThreadId, Error> {
    let prepared = worker::prepare(&shared);
    wasm_bindgen_futures::spawn_local(async move {
        prepared.await;
        let Some(run_loop) = RunLoop::adopt(&shared.run_loop) else { return };
        let slot = worker::Slot::default();
        {
            let _current = run_loop.enter();
            let _worker = worker::enter(&slot);
            worker::build(&shared, &slot);
        }
        let id = shared.id;
        // The worker holds the session from here on.
        drop(shared);
        let driven = run_loop.clone();
        crate::web::drive_session(id, run_loop, move || {
            let _current = driven.enter();
            let _worker = worker::enter(&slot);
            driven.run_once();
            if driven.is_stopped() {
                let worker = slot.borrow_mut().take();
                drop(worker);
            }
        });
    });
    Ok(std::thread::current().id())
}

/// In the browser, asks for the device that a caller-driven session's build
/// needs, as a core worker does before it builds, and wakes the host to
/// service the session once the device is there.
#[cfg(target_family = "wasm")]
fn prepare_caller_driven(shared: &Arc<SessionShared>) {
    shared.preparing.store(true, Ordering::SeqCst);
    let prepared = worker::prepare(shared);
    let shared = Arc::downgrade(shared);
    wasm_bindgen_futures::spawn_local(async move {
        prepared.await;
        if let Some(shared) = shared.upgrade() {
            shared.preparing.store(false, Ordering::SeqCst);
            shared.run_loop.invoke(|| {});
        }
    });
}

impl RenderSession {
    fn shared(&self) -> &Arc<SessionShared> {
        &self.handle.shared
    }

    /// Sends a command with `send`, such as [`barrier`](Self::barrier), and
    /// calls `callback` once with its result, where the session delivers
    /// it: on the session's thread, inside its driver call, even when the
    /// command runs before `send` returns. A [`Completion::on_complete`]
    /// after the command returned would run at once on this thread instead.
    /// The callback doesn't run when `send` fails.
    ///
    /// Fails as `send` does.
    pub fn send_then<T: Send + 'static>(
        &self,
        send: impl FnOnce(&RenderSession) -> Result<Completion<T>, Error>,
        callback: impl FnOnce(Result<T, Error>) + Send + 'static,
    ) -> Result<(), Error> {
        // The wakes that the command runs wait for the lock's release, so a
        // host's wake that services the session can't wait for it.
        with_unlocked_wakes(|| {
            let ready = {
                let _sending = self.shared().hold_sending();
                send(self)?.on_complete_unless_ready(callback)
            };
            // A result that came without the session's thread, such as a
            // command dropped unrun, goes to the callback here.
            if let Some((callback, result)) = ready {
                callback(result);
            }
            Ok(())
        })
    }

    /// Sends `task` to the session's thread, after the work sent before,
    /// resizes included, and completes with its result. A session that lost
    /// its graphics objects before the task runs completes it with
    /// [`ErrorKind::TargetLost`](mln_util::ErrorKind).
    fn operation<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Worker, &mut Outbox) -> Result<T, Error> + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.settled_operation(|worker, outbox| task(worker, outbox).map(|value| (value, None)))
    }

    /// Sends `task` as [`operation`](Self::operation) does, and completes
    /// once the host's reads that it returns end, or a second passes.
    fn settled_operation<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Worker, &mut Outbox) -> Result<(T, Pending), Error> + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        let shared = self.shared();
        with_unlocked_wakes(|| {
            let mut state = shared.state.lock();
            check_usable(&state)?;
            let (completion, resolver) = shared.completion();
            let work = state.register(resolver);
            let resizes = state.resizes_sent;
            worker::post(&shared.run_loop, move |worker, outbox| {
                let task: worker::Task = Box::new(move |worker, outbox| {
                    let Some(resolver) = worker.take_work(work) else { return };
                    match task(worker, outbox) {
                        Ok((value, pending)) => worker.settle_then(
                            pending,
                            move |outbox, settled| outbox.resolve(resolver, settled.map(|()| value)),
                            outbox,
                        ),
                        Err(error) => outbox.resolve(resolver, Err(error)),
                    }
                });
                worker.after_earlier_work(resizes, task, outbox);
            });
            drop(state);
            Ok(completion)
        })
    }

    /// Hands out the work of a [`Driver::CallerThread`] session, for the
    /// calling thread to run: the session's graphics work runs only in its
    /// [`service`](DriverWork::service), and on no other thread.
    ///
    /// Take it once, on the host's graphics thread, from the attach on: the
    /// attach itself completes in a service. It stays available after a
    /// release, which the host services to its end.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// session on a core worker, and with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once it was taken.
    pub fn take_driver_work(&self) -> Result<DriverWork, Error> {
        let shared = self.shared();
        if shared.driver != Driver::CallerThread {
            return Err(Error::unsupported("the render session runs on a core worker"));
        }
        if shared.driver_work_taken.swap(true, Ordering::SeqCst) {
            return Err(Error::invalid_state("the render session's driver work was taken already"));
        }
        let run_loop = RunLoop::adopt(&shared.run_loop)
            .ok_or_else(|| Error::invalid_state("the render session's driver work was taken already"))?;
        let _ = shared.thread.set(std::thread::current().id());
        Ok(DriverWork::new(shared.clone(), run_loop))
    }

    /// What the session can do. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once it is released.
    pub fn capabilities(&self) -> Result<RenderSessionCapabilities, Error> {
        let state = self.shared().state.lock();
        if state.released {
            return Err(released());
        }
        Ok(self.shared().capabilities)
    }

    /// The session's state now. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once it is released.
    pub fn snapshot(&self) -> Result<RenderSessionSnapshot, Error> {
        let state = self.shared().state.lock();
        if state.released {
            return Err(released());
        }
        Ok(RenderSessionSnapshot {
            state: state.lifecycle,
            extent: LogicalExtent { size: state.size, scale_factor: state.scale_factor },
            generation: state.generation,
            frame_generation: state.frame_generation,
            failure: state.failure.clone(),
            has_target: state.lifecycle == RenderSessionState::Attached && state.has_target,
            acquired_frames: state.acquired_frames,
        })
    }

    /// Takes the newest frame that a borrowed ring rendered, if one waits:
    /// the older frames that the host didn't acquire are dropped, and the
    /// session may render into their textures again. While the host holds a
    /// frame in every texture, demands wait for a release.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// session without a ring, and as a call that sends work does once the
    /// session no longer takes work.
    pub fn acquire_frame(&self) -> Result<Option<AcquiredFrame>, Error> {
        let shared = self.shared();
        let mut state = shared.state.lock();
        check_usable(&state)?;
        if !shared.capabilities.frame_acquisition {
            return Err(Error::unsupported("render session does not expose acquired texture frames"));
        }
        if state.pending_retargets != 0 {
            return Ok(None);
        }
        let newest = state
            .ring
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.acquired && slot.frame.is_some())
            .max_by_key(|(_, slot)| slot.frame.as_ref().map_or(0, |frame| frame.result.frame_generation))
            .map(|(index, _)| index);
        let Some(index) = newest else { return Ok(None) };
        for (other, slot) in state.ring.iter_mut().enumerate() {
            if other != index && !slot.acquired {
                slot.frame = None;
            }
        }
        let slot = &mut state.ring[index];
        let Some(frame) = slot.frame.take() else { return Ok(None) };
        slot.acquired = true;
        state.acquired_frames += 1;
        Ok(Some(AcquiredFrame { shared: shared.clone(), slot: index, frame, done: false }))
    }

    /// Gives a borrowed ring new textures, of `size` logical pixels: as many
    /// as it has, of its format. Once the completion reports success, the
    /// session no longer uses the old textures, and no longer waits for the
    /// consumer sync that they were given back with: the host may reuse or
    /// destroy them. The session's thread polls that sync on its later turns,
    /// for a second at most, and logs a wait that runs out. Frames wait for
    /// it meanwhile.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// session without a borrowed ring, with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for another number
    /// of textures, and with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// while the host holds one of the ring's frames. Completes with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for textures of
    /// another format or kind.
    pub fn set_ring(&self, size: Size, ring: BorrowedRing) -> Result<Completion<Applied>, Error> {
        let shared = self.shared().clone();
        {
            let mut state = shared.state.lock();
            check_usable(&state)?;
            if shared.capabilities.readback {
                return Err(Error::unsupported("an offscreen session keeps its target"));
            }
            if !shared.capabilities.frame_acquisition {
                return Err(Error::unsupported("a surface session takes no caller-owned texture"));
            }
            if shared.capabilities.export_kind.is_some() {
                return Err(Error::unsupported("an exported ring resizes instead of taking new textures"));
            }
            if state.acquired_frames != 0 {
                return Err(Error::invalid_state("cannot replace the render target while a texture frame is acquired"));
            }
            if ring.depth() != state.ring.len() {
                return Err(Error::invalid_argument(
                    "texture_count must equal the ring depth the session attached with",
                ));
            }
            state.pending_retargets += 1;
        }
        let guard = RetargetGuard(shared.clone());
        // The guard goes with the closure, whether the closure runs or the
        // resize drops it.
        self.resize_then(size, None, move |worker, size, scale_factor, outbox| {
            let _guard = guard;
            worker.set_ring(size, scale_factor, &ring, outbox)
        })
    }

    /// Asks the session to render a frame, which shows the map commands sent
    /// before the call, except the changes that an open command group holds
    /// back. A demand sent while the session attaches waits for it.
    ///
    /// Every demand finishes with one [`FrameResult`] that carries its token.
    /// A demand that isn't [`DemandMode::Force`](super::DemandMode::Force)
    /// replaces the demands with the same mode still on their way, which
    /// finish as [`FrameDisposition::Superseded`](super::FrameDisposition).
    /// While the session has no target, or a surface has no texture to
    /// render into, a demand finishes as
    /// [`FrameDisposition::TargetNotReady`](super::FrameDisposition).
    pub fn demand_frame(&self, demand: FrameDemand) -> Result<(), Error> {
        let shared = self.shared();
        with_unlocked_wakes(|| {
            let mut state = shared.state.lock();
            check_usable(&state)?;
            if !state.ring.is_empty() && state.ring.iter().all(|slot| slot.quarantined) {
                return Err(Error::invalid_state(
                    "every texture of the ring is quarantined: give the session new textures",
                ));
            }
            let sequence = state.map.as_ref().map_or(0, Map::sent_sequence);
            state.redraw_armed = true;
            worker::post(&shared.run_loop, move |worker, outbox| worker.demand(demand, sequence, outbox));
            Ok(())
        })
    }

    /// Takes the frame results that wait, oldest first. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the session is
    /// released.
    pub fn drain_frame_results(&self) -> Result<Vec<FrameResult>, Error> {
        let mut state = self.shared().state.lock();
        if state.released {
            return Err(released());
        }
        state.frame_woken = false;
        Ok(std::mem::take(&mut state.results))
    }

    /// Resizes the target, and the map with it, to `extent`'s size in
    /// logical pixels, and sets the map's scale factor to `extent`'s, such
    /// as when a window moves to a display of another density. The map keeps
    /// its tiles, and the renderer draws the next frame at the new scale
    /// factor. A later demand shows the new extent, and one sent before
    /// shows the old. An exported ring gets images of the new size, and the
    /// frames that the host holds keep theirs until it gives them back.
    ///
    /// Fails first as a call that sends work does once the session no
    /// longer takes work. Then fails with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a borrowed ring,
    /// which [`set_ring`](Self::set_ring) resizes, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty size,
    /// or a scale factor that isn't finite and positive.
    pub fn resize(&self, extent: LogicalExtent) -> Result<Completion<Applied>, Error> {
        check_usable(&self.shared().state.lock())?;
        let capabilities = self.shared().capabilities;
        if capabilities.frame_acquisition && capabilities.export_kind.is_none() {
            return Err(Error::unsupported("a borrowed ring is sized by its owner: give it new textures instead"));
        }
        let LogicalExtent { size, scale_factor } = extent;
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(Error::invalid_argument(format!("scale factor {scale_factor} isn't finite and positive")));
        }
        self.resize_then(size, Some(scale_factor), |worker, size, scale_factor, outbox| {
            worker.resize(size, scale_factor, outbox).map(|()| None)
        })
    }

    /// Sends a map command that resizes the map to `size`, at
    /// `scale_factor` or the session's when that is `None`, then runs
    /// `change` on the session's thread, which resizes or replaces the
    /// target.
    fn resize_then(
        &self,
        size: Size,
        scale_factor: Option<f64>,
        change: impl FnOnce(&mut Worker, Size, f64, &mut Outbox) -> Result<Pending, Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        if size.is_empty() {
            return Err(Error::invalid_argument("the target's size must not be empty"));
        }
        let shared = self.shared();
        with_unlocked_wakes(|| {
            let mut state = shared.state.lock();
            check_usable(&state)?;
            let map = state.map.clone().ok_or_else(|| Error::invalid_state("the render session has no map"))?;
            let scale_factor = scale_factor.unwrap_or(state.sent_scale_factor);
            state.sent_scale_factor = scale_factor;
            let (completion, resolver) = shared.completion();
            let work = state.register(resolver);
            // The work sent after waits for the resize to land, which it
            // does even when the map drops it.
            state.resizes_sent += 1;
            let landing = worker::ResizeLanding::new(shared.run_loop.clone());
            let sent =
                map.resize_for_session(size, scale_factor, shared.run_loop.clone(), move |worker, result, outbox| {
                    if let Some(resolver) = worker.take_work::<Applied>(work) {
                        match result.and_then(|applied| Ok((applied, change(worker, size, scale_factor, outbox)?))) {
                            // A replacement of a ring's textures completes
                            // once the host's reads of those it let go of end.
                            Ok((applied, pending)) => worker.settle_then(
                                pending,
                                move |outbox, settled| outbox.resolve(resolver, settled.map(|()| applied)),
                                outbox,
                            ),
                            Err(error) => outbox.resolve(resolver, Err(error)),
                        }
                    }
                    landing.land(worker, outbox);
                });
            if let Err(error) = sent {
                state.pending.remove(&work);
                return Err(error);
            }
            drop(state);
            Ok(completion)
        })
    }

    /// Lets go of a surface session's surface, and keeps the rest of the
    /// session, such as when the platform takes the window away. The demands
    /// on their way, and later ones until
    /// [`set_surface`](Self::set_surface) gives the session a surface again,
    /// finish as [`FrameDisposition::TargetNotReady`](super::FrameDisposition).
    /// Once the release completes, the session no longer uses the surface or
    /// the window it was made from, or a ring's textures, and no longer waits
    /// for the consumer sync that the textures were given back with: the host
    /// may reuse or destroy them. The session's thread polls that sync on its
    /// later turns, for a second at most, and logs a wait that runs out. The
    /// session keeps nothing of the released textures: one that a later ring
    /// has again starts fresh, even one that the host gave up.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for an
    /// offscreen session or an exported ring, whose targets are their own.
    pub fn release_target(&self) -> Result<Completion<()>, Error> {
        if self.shared().capabilities.export_kind.is_some() {
            return Err(Error::unsupported("an exported ring keeps its images until the session ends"));
        }
        let shared = self.shared().clone();
        let capabilities = shared.capabilities;
        if !capabilities.presentation && !capabilities.frame_acquisition {
            check_usable(&shared.state.lock())?;
            return Err(Error::unsupported("an offscreen session keeps its target"));
        }
        let guard = {
            let mut state = shared.state.lock();
            check_usable(&state)?;
            if state.acquired_frames != 0 {
                return Err(Error::invalid_state("release every acquired frame before the target"));
            }
            // No frame is acquired until the release runs: its textures go
            // with it.
            capabilities.frame_acquisition.then(|| {
                state.pending_retargets += 1;
                RetargetGuard(shared.clone())
            })
        };
        self.settled_operation(move |worker, outbox| {
            let _guard = guard;
            worker.release_target(outbox).map(|pending| ((), pending))
        })
    }

    /// Gives a surface session a new surface of `size` logical pixels, after
    /// [`release_target`](Self::release_target), or in place of the one it
    /// has, and resizes the map to it. Once the completion reports success,
    /// the session no longer uses the old surface. The session presents to
    /// it on its device: a window becomes a surface of the device's backend,
    /// and a `Surface::wgpu` must come from the instance of a WebGPU
    /// device. Where the backend made for the old surface can't present to
    /// the new one, as OpenGL ES's context can't to another window, the
    /// session makes its backend and renderer again.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an
    /// empty size, with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// an offscreen session, and with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a window that is
    /// minimized or has no client area. Completes with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the session's
    /// device can't present to the surface in the format that the session
    /// renders in: attach a new session instead. Completes with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the window had no
    /// area to present to by then. The session then keeps the surface it had,
    /// at the new size.
    pub fn set_surface(&self, size: Size, surface: Surface) -> Result<Completion<Applied>, Error> {
        check_usable(&self.shared().state.lock())?;
        let capabilities = self.shared().capabilities;
        if capabilities.frame_acquisition {
            return Err(Error::unsupported("a borrowed texture session takes no native surface"));
        }
        if !capabilities.presentation {
            return Err(Error::unsupported("an offscreen session keeps its target"));
        }
        surface.check_area()?;
        self.resize_then(size, None, move |worker, size, scale_factor, outbox| {
            worker.set_surface(size, scale_factor, surface, outbox).map(|()| None)
        })
    }

    /// Ends the frame demands on their way, including those that wait for a
    /// map update and the demand of a still image, which finish as
    /// [`FrameDisposition::NoUpdate`](super::FrameDisposition), and completes
    /// once the work sent before it has run. A still image that still waits
    /// gets a new demand once the barrier ran.
    pub fn barrier(&self) -> Result<Completion<()>, Error> {
        self.operation(|worker, outbox| {
            worker.barrier(outbox);
            Ok(())
        })
    }

    /// Frees memory that the renderer can rebuild, such as caches.
    pub fn reduce_memory_use(&self) -> Result<Completion<()>, Error> {
        self.operation(|worker, _| worker.reduce_memory_use())
    }

    /// Clears what the renderer holds, such as loaded tiles, so that the
    /// next frame loads and draws the map again.
    pub fn clear_data(&self) -> Result<Completion<()>, Error> {
        self.operation(Worker::clear_data)
    }

    /// Logs the renderer's sources and tiles.
    pub fn dump_debug_logs(&self) -> Result<Completion<()>, Error> {
        self.operation(|worker, _| worker.dump_debug_logs())
    }

    /// The pixels of the last frame that an offscreen session rendered:
    /// premultiplied RGBA, in rows from top to bottom, at the target's size
    /// in pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// surface session. Completes with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) when the session
    /// hasn't rendered a frame since it attached or resized.
    pub fn read_texture(&self) -> Result<Completion<PremultipliedImage>, Error> {
        check_usable(&self.shared().state.lock())?;
        if !self.shared().capabilities.readback {
            return Err(Error::unsupported("a surface session can't read its frames back"));
        }
        self.operation(|worker, _| worker.read_texture())
    }

    /// The features that the session drew in an area of the screen, in its
    /// last frame. A feature that several layers draw appears once for each.
    /// A box covers only its part inside the target, so one past every edge
    /// covers the whole target.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// point that isn't finite or a line string without points.
    pub fn query_rendered_features(
        &self,
        geometry: RenderedQueryGeometry,
        options: RenderedQueryOptions,
    ) -> Result<Completion<Vec<Feature>>, Error> {
        geometry.points()?;
        self.operation(move |worker, _| match geometry.within(worker.size()).map(|geometry| geometry.points()) {
            Some(points) => worker.renderer()?.query_rendered_features(&points?, &options),
            None => Ok(Vec::new()),
        })
    }

    /// The features of the source `source_id` in the tiles that it loaded
    /// for the current view. A source loads tiles only while a visible layer
    /// uses it.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an
    /// empty `source_id`.
    pub fn query_source_features(
        &self,
        source_id: impl Into<String>,
        options: SourceQueryOptions,
    ) -> Result<Completion<Vec<Feature>>, Error> {
        let source_id = source_id_of(source_id)?;
        self.operation(move |worker, _| worker.renderer()?.query_source_features(&source_id, &options))
    }

    /// The clusters and points that a cluster of a clustered GeoJSON source
    /// splits into at the next zoom level. `cluster_id` is a cluster
    /// feature's `cluster_id` property.
    ///
    /// The query reads the source as the session's most recent render
    /// attempt had it. A cluster ID belongs to one version of the source's
    /// data, so query the features again after the data changes.
    ///
    /// Completes with [`ErrorKind::NotFound`](mln_util::ErrorKind) when that
    /// render had no source `source_id`, or the source has no cluster
    /// `cluster_id`, and with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind)
    /// when the source isn't a clustered GeoJSON source.
    pub fn cluster_children(
        &self,
        source_id: impl Into<String>,
        cluster_id: u32,
    ) -> Result<Completion<Vec<feature::Feature<f64>>>, Error> {
        let source_id = source_id_of(source_id)?;
        self.operation(move |worker, _| worker.cluster(&source_id, cluster_id, |_, children| Ok(children)))
    }

    /// The points in a cluster of a clustered GeoJSON source: it skips the
    /// first `offset`, then holds up to `limit`. It fails as
    /// [`cluster_children`](Self::cluster_children) does.
    pub fn cluster_leaves(
        &self,
        source_id: impl Into<String>,
        cluster_id: u32,
        limit: u32,
        offset: u32,
    ) -> Result<Completion<Vec<feature::Feature<f64>>>, Error> {
        let source_id = source_id_of(source_id)?;
        self.operation(move |worker, _| {
            worker.cluster(&source_id, cluster_id, |index, _| index.get_leaves(cluster_id, limit, offset))
        })
    }

    /// The zoom level at which a cluster of a clustered GeoJSON source splits
    /// into two or more clusters or points. It fails as
    /// [`cluster_children`](Self::cluster_children) does.
    pub fn cluster_expansion_zoom(
        &self,
        source_id: impl Into<String>,
        cluster_id: u32,
    ) -> Result<Completion<u8>, Error> {
        let source_id = source_id_of(source_id)?;
        self.operation(move |worker, _| {
            worker.cluster(&source_id, cluster_id, |index, _| index.get_cluster_expansion_zoom(cluster_id))
        })
    }

    /// Detaches the session from its map and frees its graphics objects. The
    /// demands on their way finish as
    /// [`FrameDisposition::TargetNotReady`](super::FrameDisposition). Once
    /// the detach completes, another session can attach to the map.
    pub fn detach(&self) -> Result<Completion<()>, Error> {
        let shared = self.shared();
        with_unlocked_wakes(|| {
            let mut state = shared.state.lock();
            if !matches!(state.lifecycle, RenderSessionState::Failed) {
                check_usable(&state)?;
            } else if state.released {
                return Err(released());
            }
            if state.acquired_frames != 0 {
                return Err(Error::invalid_state("release every acquired frame before the session detaches"));
            }
            let (completion, resolver) = shared.completion();
            state.detach_waiters.push(resolver);
            shared.start_detach(&mut state);
            Ok(completion)
        })
    }

    /// Stops the session at once, without its pending graphics work: that
    /// work completes with [`ErrorKind::TargetLost`](mln_util::ErrorKind)
    /// before this returns, and the demands on their way finish as
    /// [`FrameDisposition::TargetNotReady`](super::FrameDisposition). Once it
    /// returns, the session renders no more, and another session can attach
    /// to the map. Release the session afterward.
    ///
    /// Returns how many graphics objects the session keeps until the
    /// process exits. A caller-driven session abandoned on the thread that
    /// services it lets go of them there, at its next service or when its
    /// [`DriverWork`] drops, and keeps none. Abandoned on another thread, it
    /// keeps them, since only that thread may free them, and logs a warning.
    ///
    /// It waits for a core worker's driver call on another thread, such as a
    /// frame that it renders. Fails with
    /// [`ErrorKind::Busy`](mln_util::ErrorKind) inside a driver call on the
    /// session's own thread, such as from a completion that the session
    /// delivers, and while a caller-driven session's service runs on another
    /// thread. Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// once the session is detached, abandoned or released.
    pub fn abandon(&self) -> Result<u32, Error> {
        self.shared().abandon(Abandoner::Host)
    }

    /// Abandons the session as [`abandon`](Self::abandon) does, but makes no
    /// graphics call: the session keeps its graphics objects until the
    /// process exits. Returns how many it keeps, such as its renderer and
    /// the backend it draws with.
    ///
    /// This is for a host whose shutdown may already have destroyed the
    /// graphics objects behind the session.
    pub fn abandon_keeping_graphics(&self) -> Result<u32, Error> {
        with_unlocked_wakes(|| self.shared().abandon_locked(Abandoner::Host, true))
    }

    /// Releases the session, detaching it first when it is attached. The
    /// completion reports when the session has let go of its map and its
    /// graphics objects. Every call on the session fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) after.
    ///
    /// A caller-driven session detaches at the host's next service, so keep
    /// servicing it until the release completes. One that isn't attached,
    /// such as while it attaches or after it failed, is abandoned instead,
    /// and so is one whose map is released before that service.
    pub fn release(&self) -> Result<Completion<()>, Error> {
        {
            let state = self.shared().state.lock();
            let live = matches!(state.lifecycle, RenderSessionState::Attaching | RenderSessionState::Attached);
            if live && !state.released && state.acquired_frames != 0 {
                return Err(Error::invalid_state("release every acquired frame before the session is released"));
            }
        }
        let (completion, resolver) = self.shared().completion();
        self.shared().dispose(Some(resolver))?;
        Ok(completion)
    }

    /// Releases the session in any state, without waiting. Dropping the last
    /// handle does the same.
    pub fn dispose(&self) -> Result<(), Error> {
        self.shared().dispose(None)
    }

    /// Releases the session in any state, without waiting and without a
    /// graphics call past the one in progress. A session that hasn't ended
    /// is abandoned, keeping its graphics objects until the process exits,
    /// as [`abandon_keeping_graphics`](Self::abandon_keeping_graphics) does.
    pub fn dispose_keeping_graphics(&self) -> Result<(), Error> {
        self.shared().dispose_keeping_graphics()
    }
}

/// Counts a ring replacement or release on its way until it runs, or never
/// will.
struct RetargetGuard(Arc<SessionShared>);

impl Drop for RetargetGuard {
    fn drop(&mut self) {
        let mut state = self.0.state.lock();
        state.pending_retargets = state.pending_retargets.saturating_sub(1);
    }
}

/// A frame of a borrowed ring that the host acquired, until it gives it
/// back. Dropping it gives it back as [`release`](Self::release) with
/// [`SyncKind::CpuComplete`] does.
pub struct AcquiredFrame {
    shared: Arc<SessionShared>,
    slot: usize,
    frame: Published,
    done: bool,
}

impl std::fmt::Debug for AcquiredFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcquiredFrame").field("slot", &self.slot).field("frame", &self.frame).finish_non_exhaustive()
    }
}

impl AcquiredFrame {
    /// The index of the frame's texture in the ring.
    #[must_use]
    pub fn slot(&self) -> usize {
        self.slot
    }

    /// The result of the demand that rendered the frame.
    #[must_use]
    pub fn result(&self) -> &FrameResult {
        &self.frame.result
    }

    /// What to wait for before reading the frame's texture. The object it
    /// names is the session's, and stays valid until the frame is given
    /// back.
    #[must_use]
    pub fn producer_sync(&self) -> GpuSync {
        self.frame.sync
    }

    /// The host's texture that holds the frame.
    #[must_use]
    pub fn texture(&self) -> Option<&FrameTexture> {
        self.frame.texture.as_ref()
    }

    /// Whether [`release`](Self::release) takes a sync of `kind`.
    #[must_use]
    pub fn accepts(&self, kind: SyncKind) -> bool {
        self.shared.capabilities.consumer_sync_kinds.contains(kind)
    }

    /// Opens a view of the frame, which keeps the frame's texture and sync
    /// objects in place until the view drops: an
    /// [`abandon`](RenderSession::abandon) fails while one is open, and a
    /// dispose of the session keeps them until the last view ends. Keep the
    /// frame until its views end.
    ///
    /// Fails with [`ErrorKind::TargetLost`](mln_util::ErrorKind) once the
    /// session can no longer render.
    pub fn view(&self) -> Result<FrameView, Error> {
        let mut state = self.shared.state.lock();
        if !matches!(
            state.lifecycle,
            RenderSessionState::Attaching | RenderSessionState::Attached | RenderSessionState::Detaching
        ) {
            return Err(Error::target_lost("the render session failed, or was detached, abandoned, or disposed"));
        }
        state.views += 1;
        Ok(FrameView { shared: self.shared.clone() })
    }

    /// Whether the session can still render: the frame's objects stay valid
    /// until it ends.
    #[must_use]
    pub fn session_live(&self) -> bool {
        matches!(
            self.shared.state.lock().lifecycle,
            RenderSessionState::Attaching | RenderSessionState::Attached | RenderSessionState::Detaching
        )
    }

    /// Gives the frame back once the host's reads of its texture reach
    /// `consumer`: the session waits for it before it renders into the
    /// texture again, and takes the sync file or GL fence that it names.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind), handing
    /// back the frame and `consumer`, for a kind that the session's
    /// [`consumer_sync_kinds`](RenderSessionCapabilities::consumer_sync_kinds)
    /// lacks.
    #[allow(clippy::result_large_err)]
    pub fn release(mut self, consumer: ConsumerSync) -> Result<(), (AcquiredFrame, ConsumerSync, Error)> {
        if !self.shared.capabilities.consumer_sync_kinds.contains(consumer.sync.kind) {
            return Err((self, consumer, Error::unsupported("render backend does not support this gpu sync kind")));
        }
        self.done = true;
        give_back(&self.shared, self.slot, Some(consumer));
        Ok(())
    }

    /// Gives the frame up without saying when the host's reads end: the
    /// session never renders into its texture again, even when
    /// [`set_ring`](RenderSession::set_ring) gives the ring the texture
    /// again, until a [`release_target`](RenderSession::release_target) of
    /// the ring.
    pub fn discard(mut self) {
        self.done = true;
        give_back(&self.shared, self.slot, None);
    }
}

/// A view of an acquired frame, which [`AcquiredFrame::view`] opens, until
/// it drops.
pub struct FrameView {
    shared: Arc<SessionShared>,
}

impl std::fmt::Debug for FrameView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameView").finish_non_exhaustive()
    }
}

impl Drop for FrameView {
    fn drop(&mut self) {
        let release = {
            let mut state = self.shared.state.lock();
            state.views = state.views.saturating_sub(1);
            state.views == 0 && std::mem::take(&mut state.viewed_teardown)
        };
        if release {
            worker::post(&self.shared.run_loop, |worker, _| worker.release_viewed());
        }
    }
}

impl Drop for AcquiredFrame {
    fn drop(&mut self) {
        if !self.done {
            give_back(&self.shared, self.slot, Some(ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))));
        }
    }
}

/// Gives the ring's `slot` back to the session, with the host's `consumer`
/// sync, or discarded without one.
fn give_back(shared: &Arc<SessionShared>, slot: usize, consumer: Option<ConsumerSync>) {
    with_unlocked_wakes(|| {
        let mut state = shared.state.lock();
        state.acquired_frames = state.acquired_frames.saturating_sub(1);
        let live = matches!(
            state.lifecycle,
            RenderSessionState::Attaching | RenderSessionState::Attached | RenderSessionState::Detaching
        );
        if !live {
            if let Some(entry) = state.ring.get_mut(slot) {
                entry.acquired = false;
                // The host's GPU may still read a texture whose wait never
                // ran.
                entry.quarantined |=
                    consumer.as_ref().is_none_or(|consumer| consumer.sync.kind != SyncKind::CpuComplete);
            }
            // A sync file that no backend waits on closes with `consumer`.
            return;
        }
        if let Some(sync) = consumer {
            worker::post(&shared.run_loop, move |worker, outbox| worker.release_slot(slot, sync, outbox));
        } else {
            if let Some(entry) = state.ring.get_mut(slot) {
                entry.acquired = false;
                entry.quarantined = !entry.retired;
                entry.retired = false;
            }
            worker::post(&shared.run_loop, move |worker, outbox| worker.discard_slot(slot, outbox));
        }
    });
}

fn source_id_of(source_id: impl Into<String>) -> Result<String, Error> {
    let source_id = source_id.into();
    if source_id.is_empty() {
        return Err(Error::invalid_argument("the source ID must not be empty"));
    }
    Ok(source_id)
}

// Handles cross threads; the renderer never leaves its own.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RenderSession>();
};
