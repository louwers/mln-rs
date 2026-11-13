// The request's state lives on the requesting thread (it holds the
// callback); file source threads reach it through `actor()`, an
// `ActorRef<FileSourceRequestState>` whose messages run in the requesting
// thread's RunLoop, as the C++ mailbox on `Scheduler::GetCurrent()` does.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{ActorRef, Mailbox, scheduler};

use crate::async_request::AsyncRequest;
use crate::file_source::Callback;
use crate::response::Response;

/// The members of `FileSourceRequest` that its messages use.
pub(crate) struct FileSourceRequestState {
    response_callback: Rc<RefCell<Callback>>,
}

impl FileSourceRequestState {
    pub(crate) fn set_response(&self, response: Response) {
        // Copy, because calling the callback will sometimes self destroy
        // this object. We cannot move because this method can be called
        // more than once.
        let callback = self.response_callback.clone();
        (callback.borrow_mut())(response);
    }
}

pub(crate) struct FileSourceRequest {
    cancel_callback: Option<Box<dyn FnOnce()>>,
    mailbox: Arc<Mailbox>,
}

impl FileSourceRequest {
    /// Creates a request that delivers responses to `callback` on the current
    /// thread's scheduler.
    #[must_use]
    pub(crate) fn new(callback: Callback) -> Self {
        let state = Rc::new(FileSourceRequestState { response_callback: Rc::new(RefCell::new(callback)) });
        let mailbox = Mailbox::with_target(&scheduler::get_current(), state);
        FileSourceRequest { cancel_callback: None, mailbox }
    }

    pub(crate) fn on_cancel(&mut self, callback: Box<dyn FnOnce()>) {
        self.cancel_callback = Some(callback);
    }

    #[must_use]
    pub(crate) fn actor(&self) -> ActorRef<FileSourceRequestState> {
        ActorRef::new(&self.mailbox)
    }
}

impl AsyncRequest for FileSourceRequest {}

impl Drop for FileSourceRequest {
    fn drop(&mut self) {
        if let Some(cancel_callback) = self.cancel_callback.take() {
            cancel_callback();
        }

        self.mailbox.close();
    }
}
