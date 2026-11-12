//! The graphics work of a render session that the host's thread runs.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mln_actor::RunLoop;
use mln_util::error::Error;
use mln_util::sync::MutexExt;

use super::session::{Abandoner, RenderSessionState, SessionShared};
use super::worker::{self, Slot, Worker};
use crate::runtime::Outbox;

/// The graphics work of a render session that attached with
/// [`Driver::CallerThread`](crate::Driver::CallerThread), which runs only
/// when the host's graphics thread calls [`service`](Self::service).
///
/// It stays on the thread that took it from
/// [`RenderSession::take_driver_work`](crate::RenderSession::take_driver_work),
/// where the session's renderer and graphics objects live. Service it each
/// time the session's
/// [`driver_work_wake`](crate::AttachOptions::driver_work_wake) runs, until
/// it [`is_finished`](Self::is_finished).
///
/// Dropping it before then abandons the session, as
/// [`RenderSession::abandon`](crate::RenderSession::abandon) does, and lets
/// go of the graphics objects on this thread.
pub struct DriverWork {
    shared: Arc<SessionShared>,
    run_loop: RunLoop,
    slot: Slot,
    servicing: Cell<bool>,
    finished: Cell<bool>,
}

impl std::fmt::Debug for DriverWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverWork").field("finished", &self.finished.get()).finish_non_exhaustive()
    }
}

fn ended() -> Error {
    Error::invalid_state("the render session ended")
}

impl DriverWork {
    pub(crate) fn new(shared: Arc<SessionShared>, run_loop: RunLoop) -> DriverWork {
        DriverWork { shared, run_loop, slot: Slot::default(), servicing: Cell::new(false), finished: Cell::new(false) }
    }

    /// Runs the session's work that is ready, and returns once none is: the
    /// attach, frames, the work that the handle sent, and the detach. The
    /// first service builds the session's renderer. The session's
    /// completions and wakes may run inside it, on this thread.
    ///
    /// Fails with [`ErrorKind::Busy`](mln_util::ErrorKind) inside a service
    /// that is running, which takes the work instead, and with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the session
    /// ended before the call: its detach or release completed, or it was
    /// abandoned. A service of an abandoned session lets go of its graphics
    /// objects before it fails.
    pub fn service(&self) -> Result<(), Error> {
        if self.finished.get() {
            return Err(ended());
        }
        if self.servicing.replace(true) {
            return Err(Error::busy("the render session's driver work is running on this thread"));
        }
        let abandoned = self.shared.state.lock_unpoisoned().lifecycle == RenderSessionState::Abandoned;
        self.shared.driver_armed.store(true, Ordering::SeqCst);
        {
            let _current = self.run_loop.enter();
            let _worker = worker::enter(&self.slot);
            worker::build(&self.shared, &self.slot);
            while self.run_loop.run_once() {}
            if abandoned || self.slot.borrow().as_ref().is_some_and(Worker::is_done) {
                self.end();
            }
        }
        self.servicing.set(false);
        if abandoned { Err(ended()) } else { Ok(()) }
    }

    /// Whether the session ended, so the host needn't service it again.
    pub fn is_finished(&self) -> bool {
        self.finished.get()
    }

    /// Lets go of the session's renderer and graphics objects, with the
    /// session's loop current.
    fn end(&self) {
        self.finished.set(true);
        self.shared.run_loop.set_waker(None);
        let mut outbox = Outbox::default();
        let gate = self.shared.gate.lock_unpoisoned();
        let worker = self.slot.borrow_mut().take();
        if let Some(worker) = worker {
            worker.close(&mut outbox);
        }
        drop(gate);
        outbox.deliver();
    }
}

impl Drop for DriverWork {
    fn drop(&mut self) {
        if self.finished.get() {
            return;
        }
        let _ = self.shared.abandon(Abandoner::Library);
        let _current = self.run_loop.enter();
        let _worker = worker::enter(&self.slot);
        self.end();
    }
}
