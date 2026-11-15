//! The browser build: everything of a runtime on the one thread of its wasm
//! instance, as examples/web runs a map.
//!
//! The instance has no other threads, so a runtime's loop and each render
//! session's loop run on the thread that made them, in the slices of the
//! instance's pump (`mln_web_host::pump`), which the loops and the
//! cooperative scheduler wake. Nothing waits: a command runs in a later
//! slice, never inside the call that sent it. Each session's renderer runs
//! its tiles' jobs in Web Workers of the instance's own ([`tile_workers`]),
//! and shapes font faces with the shaper module
//! (the runtime's font-face engine) once a style declares them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_actor::RunLoop;
use mln_renderer::tile::worker_codec::RemoteWorkers;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_web_host::{pump, tile_workers};

use crate::render::{SessionId, SessionQueue};
use crate::runtime::{CoreQueue, RuntimeCore};

/// Makes the instance ready for runtimes: the cooperative scheduler, which
/// the pump runs.
pub(crate) fn install() {
    let scheduler = mln_web_host::install_cooperative_scheduler();
    scheduler.set_waker(Some(pump::waker()));
}

/// Runs a runtime's `queue` with its `core` in the pump's slices until the
/// queue stops, and with it the runtime's loop, `run_loop`, which the
/// runtime's maps run their own work on.
pub(crate) fn drive(run_loop: &RunLoop, queue: CoreQueue, mut core: Option<RuntimeCore>) {
    let run_loop = run_loop.clone();
    run_loop.set_waker(Some(pump::waker()));
    queue.set_waker(Some(pump::waker()));
    pump::add(move || {
        run_loop.run_once();
        queue.run_once(&mut core);
        if queue.is_stopped() {
            drop(core.take());
        }
        !queue.is_stopped()
    })
    .detach();
}

/// How a session's loop runs on this thread.
struct SessionLoop {
    run: Box<dyn Fn()>,
    running: Cell<bool>,
}

thread_local! {
    /// The loops of the sessions on core workers, which
    /// [`run_session_now`] finds by their session's id.
    static SESSIONS: RefCell<HashMap<SessionId, Rc<SessionLoop>>> = RefCell::new(HashMap::default());
}

/// Runs a core worker session in the pump's slices until its `queue`
/// stops, with `run` once per slice, and lets [`run_session_now`] run it too.
/// `run_loop` is the session's renderer's.
pub(crate) fn drive_session(id: SessionId, run_loop: &RunLoop, queue: SessionQueue, run: impl Fn() + 'static) {
    run_loop.set_waker(Some(pump::waker()));
    queue.set_waker(Some(pump::waker()));
    let session = Rc::new(SessionLoop { run: Box::new(run), running: Cell::new(false) });
    SESSIONS.with(|sessions| sessions.borrow_mut().insert(id, Rc::clone(&session)));
    pump::add(move || {
        run_session(&session);
        let stopped = queue.is_stopped();
        if stopped {
            SESSIONS.with(|sessions| sessions.borrow_mut().remove(&id));
        }
        !stopped
    })
    .detach();
}

fn run_session(session: &SessionLoop) {
    if !session.running.replace(true) {
        (session.run)();
        session.running.set(false);
    }
}

/// Runs a core worker session's loop once now, unless it runs already or
/// has yet to start: what a thread that waits for a session's next task
/// does elsewhere.
pub(crate) fn run_session_now(id: SessionId) {
    let session = SESSIONS.with(|sessions| sessions.borrow().get(&id).cloned());
    if let Some(session) = session {
        run_session(&session);
    }
}

/// Tile workers for a renderer on `scheduler`'s loop, in the instance's tile
/// worker Web Workers, or `None` where none start, which leaves the tiles'
/// jobs on the cooperative scheduler, in the pump's slices. A Web Worker
/// that panics is replaced by a new one.
pub(crate) fn tile_workers(scheduler: &mln_actor::SchedulerPtr) -> Option<Rc<RemoteWorkers>> {
    let ports = match tile_workers::connect() {
        Ok(ports) => ports,
        Err(error) => {
            Log::warning(
                Event::General,
                format!("The tile workers didn't start, so tiles load on this thread: {error}"),
            );
            return None;
        }
    };
    let fatal = |instance: usize, message: wasm_bindgen::JsValue| {
        let message = message.as_string().unwrap_or_else(|| format!("{message:?}"));
        Log::error(Event::General, format!("Tile worker {instance} panicked: {message}"));
    };
    let respawn = |instance: usize| {
        tile_workers::respawn(instance)
            .inspect_err(|error| Log::error(Event::General, format!("No tile worker took {instance}'s place: {error}")))
            .ok()
    };
    Some(tile_workers::workers(scheduler, ports, |_| {}, fatal, respawn))
}
