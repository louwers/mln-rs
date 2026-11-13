//! The browser build: everything of a runtime on the one thread of its wasm
//! instance, as examples/web runs a map.
//!
//! The instance has no other threads, so a runtime's loop and each render
//! session's loop run on the thread that made them, in the slices of the
//! instance's pump (`mln_web_host::pump`), which the loops and the
//! cooperative scheduler wake. Nothing waits: a command runs in a later
//! slice, never inside the call that sent it. Each session's renderer puts
//! its tiles' workers in Web Workers of the instance's own
//! ([`tile_worker_transport`]), and shapes font faces with the shaper module
//! once a style declares them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_renderer::text::font_face_engine;
use mln_renderer::tile::worker_codec::SerializingTransport;
use mln_util::logging::{Event, Log};
use mln_web_host::{pump, shaper, tile_workers};

use crate::render::SessionId;

/// Makes the instance ready for runtimes: the cooperative scheduler, which
/// the pump runs, and the font-face shaper module.
pub(crate) fn install() {
    let scheduler = mln_web_host::install_cooperative_scheduler();
    scheduler.set_waker(Some(pump::waker()));
    thread_local! {
        static SHAPER: Cell<bool> = const { Cell::new(false) };
    }
    if !SHAPER.replace(true) {
        font_face_engine::set_font_face_engine(Arc::new(shaper::ShaperModuleEngine));
    }
}

/// Runs `run_loop` in the pump's slices until it stops. Each slice calls
/// `around` with the loop current and a function that runs it once, so
/// that `around` can put what the loop's tasks need in place.
pub(crate) fn drive(run_loop: RunLoop, mut around: impl FnMut(&dyn Fn()) + 'static) {
    run_loop.set_waker(Some(pump::waker()));
    pump::add(move || {
        let _current = run_loop.enter();
        around(&|| {
            run_loop.run_once();
        });
        !run_loop.is_stopped()
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
    static SESSIONS: RefCell<HashMap<SessionId, Rc<SessionLoop>>> = RefCell::new(HashMap::new());
}

/// Runs a core worker session's loop in the pump's slices until it stops,
/// with `run` once per slice, and lets [`run_session_now`] run it too.
pub(crate) fn drive_session(id: SessionId, run_loop: RunLoop, run: impl Fn() + 'static) {
    run_loop.set_waker(Some(pump::waker()));
    let session = Rc::new(SessionLoop { run: Box::new(run), running: Cell::new(false) });
    SESSIONS.with(|sessions| sessions.borrow_mut().insert(id, session.clone()));
    pump::add(move || {
        run_session(&session);
        let stopped = run_loop.is_stopped();
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

/// A transport that puts the tiles' workers of a renderer on this thread's
/// current loop in the instance's tile worker Web Workers, or `None` where
/// none start, which leaves the workers on the cooperative scheduler, in the
/// pump's slices.
pub(crate) fn tile_worker_transport() -> Option<Rc<SerializingTransport>> {
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
    Some(tile_workers::transport(ports, |_| {}, fatal))
}
