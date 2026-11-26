//! The browser build: everything of a runtime on the one thread of its wasm
//! instance, as examples/web runs a map.
//!
//! The instance has no other threads, so a runtime's queue and pool, its
//! network loading's queue and each render session's queue run on the
//! thread that made them, in the slices of
//! the instance's pump (`mln_web_host::pump`), which the queues and the pool
//! wake. Nothing waits: a command runs in a later slice, never inside the
//! call that sent it. Each session's renderer runs
//! its tiles' jobs in Web Workers of the instance's own ([`tile_workers`]),
//! and shapes font faces with the shaper module
//! (the runtime's font-face engine) once a style declares them.
//!
//! JavaScript values stay on this thread with what owns them: the fetch
//! preloads with the network loading's core, which the pump runs, and the
//! canvases that a session's attach and surface changes hand its worker in
//! the session's side here, until the worker takes them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_renderer::Registry;
use mln_renderer::tile::worker_codec::RemoteWorkers;
use mln_storage::OnlineFileSource;
use mln_storage::network_status::NetworkStatus;
use mln_style::hash::HashMap;
use mln_util::pool::Pool;
use mln_web_host::{pump, tile_workers};

use crate::log_target;
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
        } else {
            wake_for_timed(&queue);
        }
        !stopped
    })
    .detach();
}

/// Asks the pump for a slice once `queue`'s earliest timed task is due.
fn wake_for_timed<S>(queue: &mln_util::queue::Queue<S>) {
    if let Some(deadline) = queue.next_deadline() {
        pump::wake_at(deadline);
    }
}

/// The runtime's own network loading, which looks for the host's responses
/// fetched ahead with `fetch_preloads` first. Its core's queue runs in the
/// pump's slices until the source drops, and in a slice of its own once a
/// retry or revalidation is due.
pub(crate) fn online(network: Arc<NetworkStatus>, fetch_preloads: Option<js_sys::Function>) -> OnlineFileSource {
    let (online, queue, mut core) = OnlineFileSource::local(network);
    core.set_fetch_preloads(fetch_preloads);
    queue.set_waker(Some(pump::waker()));
    let mut core = Some(core);
    pump::add(move || {
        let Some(state) = &mut core else { return false };
        queue.run_once(state);
        let stopped = queue.is_stopped();
        if stopped {
            queue.close();
            drop(core.take());
        } else {
            wake_for_timed(&queue);
        }
        !stopped
    })
    .detach();
    online
}

/// How a core worker session's queue runs on this thread.
struct SessionLoop {
    run: Box<dyn Fn()>,
    running: Cell<bool>,
}

/// What this thread holds of a session: how a core worker's queue runs,
/// which [`run_session_now`] finds by the session's id, and the canvases
/// that the session's attach and surface changes hand its worker, until it
/// takes them.
#[derive(Default)]
struct SessionSide {
    run: Option<Rc<SessionLoop>>,
    canvases: HashMap<u64, wasm_bindgen::JsValue>,
}

/// The sessions' sides on this thread, by the session's id.
#[derive(Default)]
struct Sessions {
    sides: HashMap<SessionId, SessionSide>,
    /// The key of the next canvas handed to a session.
    next_canvas: u64,
}

impl Sessions {
    /// Drops `id`'s side once it holds nothing.
    fn prune(&mut self, id: SessionId) {
        if self.sides.get(&id).is_some_and(|side| side.run.is_none() && side.canvases.is_empty()) {
            self.sides.remove(&id);
        }
    }
}

thread_local! {
    static SESSIONS: RefCell<Sessions> = RefCell::default();
}

/// Runs `f` with the sessions' sides, unless this thread is ending. `f`
/// must not call out of this module.
fn with_sessions<R>(f: impl FnOnce(&mut Sessions) -> R) -> Option<R> {
    SESSIONS.try_with(|sessions| f(&mut sessions.borrow_mut())).ok()
}

/// A canvas handed to a session's worker: its key in the session's side,
/// where it waits on this thread until the worker [`take`](Self::take)s it
/// or this drops.
pub(crate) struct HandedCanvas {
    session: SessionId,
    key: u64,
}

/// Hands `canvas` to the worker of session `session`, which runs on this
/// thread.
pub(crate) fn hand_canvas(session: SessionId, canvas: wasm_bindgen::JsValue) -> HandedCanvas {
    let key = with_sessions(|sessions| {
        let key = sessions.next_canvas;
        sessions.next_canvas += 1;
        sessions.sides.entry(session).or_default().canvases.insert(key, canvas);
        key
    });
    // A thread that is ending hands nothing over: the key names no canvas.
    HandedCanvas { session, key: key.unwrap_or(u64::MAX) }
}

impl HandedCanvas {
    /// The canvas, unless this thread is ending.
    pub(crate) fn take(self) -> Option<wasm_bindgen::JsValue> {
        self.remove()
    }

    fn remove(&self) -> Option<wasm_bindgen::JsValue> {
        with_sessions(|sessions| {
            let canvas = sessions.sides.get_mut(&self.session)?.canvases.remove(&self.key);
            sessions.prune(self.session);
            canvas
        })
        .flatten()
    }
}

impl Drop for HandedCanvas {
    fn drop(&mut self) {
        drop(self.remove());
    }
}

/// Runs a core worker session in the pump's slices until its `queue`
/// stops, with `run` once per slice, and lets [`run_session_now`] run it too.
pub(crate) fn drive_session(id: SessionId, queue: SessionQueue, run: impl Fn() + 'static) {
    queue.set_waker(Some(pump::waker()));
    let session = Rc::new(SessionLoop { run: Box::new(run), running: Cell::new(false) });
    with_sessions(|sessions| sessions.sides.entry(id).or_default().run = Some(Rc::clone(&session)));
    pump::add(move || {
        run_session(&session);
        let stopped = queue.is_stopped();
        if stopped {
            with_sessions(|sessions| {
                if let Some(side) = sessions.sides.get_mut(&id) {
                    side.run = None;
                }
                sessions.prune(id);
            });
        } else {
            wake_for_timed(&queue);
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
    let session = with_sessions(|sessions| sessions.sides.get(&id).and_then(|side| side.run.clone())).flatten();
    if let Some(session) = session {
        run_session(&session);
    }
}

/// Tile workers for a renderer, in the instance's tile worker Web Workers,
/// whose jobs name `registry`'s registrations, or `None` where none start,
/// which leaves the tiles' jobs on the runtime's pool, in the pump's slices.
/// A Web Worker that panics is replaced by a new one.
pub(crate) fn tile_workers(registry: &Arc<Registry>) -> Option<RemoteWorkers> {
    let ports = match tile_workers::connect() {
        Ok(ports) => ports,
        Err(error) => {
            log::warn!(
                target: log_target::GENERAL,
                "The tile workers didn't start, so tiles load on this thread: {error}"
            );
            return None;
        }
    };
    let fatal = |instance: usize, message: wasm_bindgen::JsValue| {
        let message = message.as_string().unwrap_or_else(|| format!("{message:?}"));
        log::error!(target: log_target::GENERAL, "Tile worker {instance} panicked: {message}");
    };
    let respawn = |instance: usize| {
        tile_workers::respawn(instance)
            .inspect_err(
                |error| log::error!(target: log_target::GENERAL, "No tile worker took {instance}'s place: {error}"),
            )
            .ok()
    };
    Some(tile_workers::workers(ports, Arc::clone(registry), fatal, respawn))
}
