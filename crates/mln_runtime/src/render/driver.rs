//! The graphics work of a render session that the host's thread runs.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mln_actor::RunLoop;
use mln_util::error::Error;

use super::session::{Abandoner, RenderSessionState, SessionShared};
use super::worker::{self, Worker};
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
    /// The loop that the session's renderer runs its own work on, of the
    /// session's own: a thread may service several sessions.
    run_loop: RunLoop,
    /// The session's worker, once the first service built it.
    worker: RefCell<Option<Worker>>,
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
    pub(crate) fn new(shared: Arc<SessionShared>) -> DriverWork {
        let run_loop = RunLoop::new();
        // The renderer's work wakes the host as the session's does.
        let queue = shared.queue.clone();
        run_loop.set_waker(Some(Arc::new(move || queue.wake())));
        DriverWork {
            shared,
            run_loop,
            worker: RefCell::new(None),
            servicing: Cell::new(false),
            finished: Cell::new(false),
        }
    }

    /// Runs the session's work that is ready, and returns once none is: the
    /// attach, frames, the work that the handle sent, and the detach. The
    /// first service builds the session's renderer: in the browser, the first
    /// once the device that the session asked WebGPU for is there. The
    /// session's completions and other wakes may run inside it, on this
    /// thread, but its driver work wake doesn't: the service takes the work
    /// that comes while it runs. Work that is ready after it runs the wake
    /// again.
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
        let abandoned = self.shared.state.lock().lifecycle == RenderSessionState::Abandoned;
        let armed = &self.shared.driver_armed;
        if !abandoned && self.shared.preparing.load(Ordering::SeqCst) {
            armed.store(true, Ordering::SeqCst);
            // The session's device is on its way, and the wake runs once it
            // is here.
            self.servicing.set(false);
            return Ok(());
        }
        {
            let _call = self.shared.enter_call();
            let queue = &self.shared.queue;
            // The wake waits while the service runs, and the service runs
            // the work that comes meanwhile, so the wake never comes on this
            // thread during a service. It's armed again only once the loop
            // is empty; work that came just before that runs here too.
            armed.store(false, Ordering::SeqCst);
            let mut worker = self.worker.borrow_mut();
            worker::build(&self.shared, &mut worker, &self.run_loop);
            loop {
                while self.run_loop.run_once() | queue.run_once(&mut worker) {}
                armed.store(true, Ordering::SeqCst);
                if !self.run_loop.has_pending() && !queue.has_pending() {
                    break;
                }
                armed.store(false, Ordering::SeqCst);
            }
            let done = worker.as_ref().is_some_and(Worker::is_done);
            drop(worker);
            if abandoned || done {
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

    /// Lets go of the session's renderer and graphics objects, then of the
    /// driver-work wake: its release tells the host that the session makes
    /// no more graphics calls.
    fn end(&self) {
        self.finished.set(true);
        let mut outbox = Outbox::default();
        let gate = self.shared.gate.lock();
        let worker = self.worker.borrow_mut().take();
        if let Some(worker) = worker {
            worker.close(&mut outbox);
        }
        drop(gate);
        self.shared.queue.set_waker(None);
        self.run_loop.set_waker(None);
        self.shared.wait_for_senders();
        outbox.deliver();
    }
}

impl Drop for DriverWork {
    fn drop(&mut self) {
        if self.finished.get() {
            return;
        }
        let _ = self.shared.abandon(Abandoner::Library);
        self.end();
    }
}
