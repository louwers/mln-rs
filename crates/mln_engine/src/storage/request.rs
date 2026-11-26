//! A request for a resource, and the file source's side of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::storage::response::Response;

/// Receives a request's responses, on the thread that sends each one.
pub type Callback = Box<dyn FnMut(Response) + Send + 'static>;

/// A request for a resource. Dropping it cancels the request: the file
/// source stops what work it can, and no response reaches the callback
/// after the drop. The drop doesn't wait for a call that began before it,
/// on the responder's thread, so a callback may drop its own request.
#[must_use = "dropping a request cancels it"]
pub struct Request {
    shared: Arc<Shared>,
}

/// The file source's side of a [`Request`]: where it sends the responses,
/// from any thread, and how it learns that the requester cancelled.
///
/// Dropping it ends the request without a response; a file source that
/// can't load a resource sends a response with an error instead.
#[derive(Clone)]
pub struct Responder {
    shared: Arc<Shared>,
}

struct Shared {
    cancelled: AtomicBool,
    /// The first response ends the request.
    once: bool,
    on_cancel: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// `None` once the request dropped while no response was being sent.
    callback: Mutex<Option<Callback>>,
}

impl Shared {
    fn new(callback: Callback, once: bool) -> Arc<Self> {
        Arc::new(Shared {
            cancelled: AtomicBool::new(false),
            once,
            on_cancel: Mutex::new(None),
            callback: Mutex::new(Some(callback)),
        })
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let on_cancel = self.on_cancel.lock().take();
        if let Some(on_cancel) = on_cancel {
            on_cancel();
        }
    }
}

impl Request {
    /// A request whose responses `callback` receives on the thread that sends
    /// each one.
    pub fn new(callback: impl FnMut(Response) + Send + 'static) -> (Request, Responder) {
        let shared = Shared::new(Box::new(callback), false);
        (Request { shared: Arc::clone(&shared) }, Responder { shared })
    }

    /// A request for one response, which `callback` receives on the thread
    /// that sends it. The request ends with it: the file source sees it
    /// cancelled and sends nothing more, though its cancel hook runs only
    /// when the request drops.
    pub fn once(callback: impl FnOnce(Response) + Send + 'static) -> (Request, Responder) {
        let mut callback = Some(callback);
        let shared = Shared::new(
            Box::new(move |response| {
                if let Some(callback) = callback.take() {
                    callback(response);
                }
            }),
            true,
        );
        (Request { shared: Arc::clone(&shared) }, Responder { shared })
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.shared.cancel();
        // While a response is being sent, its callback goes when the
        // responders drop, rather than the drop waiting for it.
        let callback = self.shared.callback.try_lock().and_then(|mut callback| callback.take());
        drop(callback);
    }
}

impl Responder {
    /// Sends a response to the callback, on this thread. It does nothing
    /// once the requester cancelled.
    pub fn send(&self, response: Response) {
        if self.is_cancelled() {
            return;
        }
        let mut callback = self.shared.callback.lock();
        // The request may have dropped while this waited for the lock.
        if self.is_cancelled() {
            return;
        }
        if self.shared.once {
            self.shared.cancelled.store(true, Ordering::SeqCst);
        }
        if let Some(callback) = callback.as_mut() {
            callback(response);
        }
    }

    /// Whether the requester cancelled the request.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shared.is_cancelled()
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
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn counter() -> (Arc<AtomicUsize>, impl FnMut(Response) + Send + 'static) {
        let count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&count);
        (count, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        })
    }

    /// A request for one response ends at it: the responder sees it
    /// cancelled, and a second response goes nowhere.
    #[test]
    fn a_once_request_ends_at_its_response() {
        let (responses, callback) = counter();
        let (request, responder) = Request::once(callback);
        assert!(!responder.is_cancelled());
        responder.send(Response::new());
        assert!(responder.is_cancelled());
        responder.send(Response::new());
        assert_eq!(responses.load(Ordering::SeqCst), 1);
        drop(request);
    }

    /// No response reaches the callback after the request drops, and the
    /// drop releases the callback without waiting for the responders.
    #[test]
    fn a_dropped_request_answers_nothing_more() {
        let (responses, callback) = counter();
        let (request, responder) = Request::new(callback);
        responder.send(Response::new());
        drop(request);
        responder.send(Response::new());
        assert_eq!(responses.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&responses), 1, "the callback went with the request");
    }

    /// A callback may drop its own request, which cancels it without
    /// waiting for the call that drops it.
    #[test]
    fn a_callback_drops_its_own_request() {
        let slot: Arc<Mutex<Option<Request>>> = Arc::default();
        let held = Arc::clone(&slot);
        let (request, responder) = Request::new(move |_| drop(held.lock().take()));
        *slot.lock() = Some(request);
        responder.send(Response::new());
        assert!(responder.is_cancelled());
        assert!(slot.lock().is_none());
    }
}
