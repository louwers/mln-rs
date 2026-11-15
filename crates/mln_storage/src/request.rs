//! A request for a resource, and the file source's side of it.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use mln_actor::{LocalObject, Scheduler, SchedulerPtr};
use parking_lot::Mutex;

use crate::response::Response;

/// Receives a request's responses on the thread that made the request, so it
/// need not be `Send`.
pub type Callback = Box<dyn FnMut(Response) + 'static>;

/// The requesting thread's queue, which a request doesn't keep alive.
type WeakQueue = Weak<dyn Scheduler + Send + Sync>;

/// The callback of a request that answers on the sending thread.
type DirectCallback = Arc<Mutex<Box<dyn FnMut(Response) + Send>>>;

/// A request for a resource. Dropping it cancels the request: its callback
/// doesn't run after that, and the file source stops what work it can.
#[must_use = "dropping a request cancels it"]
pub struct Request {
    shared: Arc<Shared>,
    /// The callback on the requesting thread, which the drop removes, and
    /// that thread's queue.
    slot: Option<(Arc<LocalObject<Slot>>, WeakQueue)>,
}

/// The file source's side of a [`Request`]: where it sends the responses,
/// from any thread, and how it learns that the requester cancelled.
///
/// Dropping it ends the request without a response; a file source that
/// can't load a resource sends a response with an error instead.
#[derive(Clone)]
pub struct Responder {
    shared: Arc<Shared>,
    deliver: Deliver,
}

#[derive(Clone)]
enum Deliver {
    /// In a task on the requesting thread's queue.
    Queued { queue: Weak<dyn Scheduler + Send + Sync>, slot: Arc<LocalObject<Slot>> },
    /// On the sending thread.
    Direct(DirectCallback),
}

/// The callback of a queued request, on the requesting thread.
struct Slot(RefCell<Callback>);

impl Slot {
    /// Runs the callback, unless the request was cancelled, or the callback
    /// runs already: then it runs once that call returns, in a task of
    /// `queue`.
    fn call(
        shared: &Arc<Shared>,
        slot: &Arc<LocalObject<Slot>>,
        queue: &Weak<dyn Scheduler + Send + Sync>,
        response: Response,
    ) {
        if shared.cancelled.load(Ordering::SeqCst) {
            return;
        }
        let Some(target) = slot.get() else { return };
        let Ok(mut callback) = target.0.try_borrow_mut() else {
            Self::queue(shared, slot, queue, response);
            return;
        };
        callback(response);
    }

    fn queue(
        shared: &Arc<Shared>,
        slot: &Arc<LocalObject<Slot>>,
        queue: &Weak<dyn Scheduler + Send + Sync>,
        response: Response,
    ) {
        let Some(scheduler) = queue.upgrade() else { return };
        let (shared, slot, weak) = (Arc::clone(shared), Arc::clone(slot), Weak::clone(queue));
        scheduler.schedule(Box::new(move || Slot::call(&shared, &slot, &weak, response)));
    }
}

#[derive(Default)]
struct Shared {
    cancelled: AtomicBool,
    on_cancel: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Shared {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let on_cancel = self.on_cancel.lock().take();
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }
}

impl Request {
    /// A request whose responses `callback` receives on the thread that runs
    /// `queue`, which must be this thread.
    pub fn new(queue: &SchedulerPtr, callback: impl FnMut(Response) + 'static) -> (Request, Responder) {
        let slot = Arc::new(LocalObject::new(Slot(RefCell::new(Box::new(callback)))));
        let shared = Arc::new(Shared::default());
        let deliver = Deliver::Queued { queue: Arc::downgrade(queue), slot: Arc::clone(&slot) };
        (
            Request { shared: Arc::clone(&shared), slot: Some((slot, Arc::downgrade(queue))) },
            Responder { shared, deliver },
        )
    }

    /// A request whose responses `callback` receives on the thread that sends
    /// them, for work that continues wherever its responses arrive.
    pub fn direct(callback: impl FnMut(Response) + Send + 'static) -> (Request, Responder) {
        let shared = Arc::new(Shared::default());
        let deliver = Deliver::Direct(Arc::new(Mutex::new(Box::new(callback))));
        (Request { shared: Arc::clone(&shared), slot: None }, Responder { shared, deliver })
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.shared.cancel();
        if let Some((slot, queue)) = self.slot.take() {
            // Dropped on another thread, the callback goes on its own.
            if slot.take().is_none()
                && let Some(queue) = queue.upgrade()
            {
                queue.schedule(Box::new(move || drop(slot.take())));
            }
        }
    }
}

impl Responder {
    /// Sends a response. It does nothing once the requester cancelled.
    pub fn send(&self, response: Response) {
        if self.is_cancelled() {
            return;
        }
        match &self.deliver {
            Deliver::Queued { queue, slot } => Slot::queue(&self.shared, slot, queue, response),
            Deliver::Direct(callback) => (callback.lock())(response),
        }
    }

    /// Sends a response, and on the requesting thread, runs the callback
    /// before it returns: for a file source that runs on the requesting
    /// thread, such as a test's. Elsewhere, and while the callback runs
    /// already, it's [`send`](Self::send).
    pub fn send_now(&self, response: Response) {
        if self.is_cancelled() {
            return;
        }
        match &self.deliver {
            Deliver::Queued { queue, slot } => Slot::call(&self.shared, slot, queue, response),
            Deliver::Direct(callback) => (callback.lock())(response),
        }
    }

    /// Whether the requester cancelled the request.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    /// Runs `on_cancel` when the requester cancels the request, on the thread
    /// that cancels it, or now if it has. It replaces the one before.
    pub fn on_cancel(&self, on_cancel: impl FnOnce() + Send + 'static) {
        *self.shared.on_cancel.lock() = Some(Box::new(on_cancel));
        // The request may have been cancelled before `on_cancel` was there.
        if self.is_cancelled() {
            let on_cancel = self.shared.on_cancel.lock().take();
            if let Some(on_cancel) = on_cancel {
                on_cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use mln_actor::RunLoop;

    use super::*;

    /// A request dropped on another thread still cancels the responses that
    /// were queued before the drop.
    #[test]
    fn a_request_dropped_elsewhere_runs_no_queued_response() {
        let run_loop = RunLoop::new();
        let ran = Rc::new(Cell::new(false));
        let callback_ran = Rc::clone(&ran);
        let (request, responder) = Request::new(&run_loop.scheduler(), move |_| callback_ran.set(true));
        responder.send(Response::new());
        std::thread::spawn(move || drop(request)).join().unwrap();
        while run_loop.run_once() {}
        assert!(!ran.get());
    }
}
