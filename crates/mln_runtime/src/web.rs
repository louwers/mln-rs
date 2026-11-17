//! The browser build: everything of a runtime on the one thread of its wasm
//! instance, as examples/web runs a map.
//!
//! The instance has no other threads, so a runtime's queue and pool and each
//! render session's queue run on the thread that made them, in the slices of
//! the instance's pump (`mln_web_host::pump`), which the queues and the pool
//! wake. Nothing waits: a command runs in a later slice, never inside the
//! call that sent it. Each session's renderer runs
//! its tiles' jobs in Web Workers of the instance's own ([`tile_workers`]),
//! and shapes font faces with the shaper module
//! (the runtime's font-face engine) once a style declares them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_renderer::Registry;
use mln_renderer::tile::worker_codec::{Post, RemoteWorkers};
use mln_storage::OnlineFileSource;
use mln_storage::network_status::NetworkStatus;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::pool::Pool;
use mln_web_host::{pump, tile_workers};

use crate::render::{SessionId, SessionQueue};
use crate::runtime::{CoreQueue, RuntimeCore};

/// Runs a runtime's `queue` with its `core` in the pump's slices until the
/// queue stops, and with it its `pool`'s tasks.
pub(crate) fn drive(pool: &Pool, queue: CoreQueue, mut core: Option<RuntimeCore>) {
    queue.set_waker(Some(pump::waker()));
    pool.set_waker(Some(pump::waker()));
    let mut tasks = Some(pump::add_pool(pool.clone()));
    pump::add(move || {
        queue.run_once(&mut core);
        let stopped = queue.is_stopped();
        if stopped {
            queue.close();
            drop(core.take());
            drop(tasks.take());
        }
        !stopped
    })
    .detach();
}

/// The runtime's own network loading, whose core runs in the pump's slices
/// until the source drops.
pub(crate) fn online(network: Arc<NetworkStatus>) -> OnlineFileSource {
    let (online, mut core) = OnlineFileSource::local(network, pump::waker());
    pump::add(move || core.run_due()).detach();
    online
}

/// How a session's queue runs on this thread.
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
pub(crate) fn drive_session(id: SessionId, queue: SessionQueue, run: impl Fn() + 'static) {
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

/// Runs a core worker session's queue once now, unless it runs already or
/// has yet to start: what a thread that waits for a session's next task
/// does elsewhere.
pub(crate) fn run_session_now(id: SessionId) {
    let session = SESSIONS.with(|sessions| sessions.borrow().get(&id).cloned());
    if let Some(session) = session {
        run_session(&session);
    }
}

/// Tile workers for a renderer whose thread runs what `post` posts, in the
/// instance's tile worker Web Workers, whose jobs name `registry`'s
/// registrations, or `None` where none start, which leaves the tiles' jobs
/// on the runtime's pool, in the pump's slices. A Web Worker that panics is
/// replaced by a new one.
pub(crate) fn tile_workers(post: Post, registry: &Arc<Registry>) -> Option<Rc<RemoteWorkers>> {
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
    Some(tile_workers::workers(post, ports, Arc::clone(registry), |_| {}, fatal, respawn))
}
