// A stub answers each request with the answer for its resource: the one
// for every kind if it has one, else the one for the resource's kind at the
// time it answers, so that an answer replaced while a request waits (as
// after an error response) answers the retry.

use std::sync::{Arc, Weak};

use mln_style::hash::HashMap;
use mln_vec_map::VecMap;
use parking_lot::Mutex;

use crate::exec::queue::Queue;
use crate::storage::file_source::FileSource;
use crate::storage::request::Responder;
use crate::storage::resource::{Kind, Resource};
use crate::storage::response::Response;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseType {
    /// Requests wait for a [`poll`](StubFileSource::poll), which the waker
    /// asks for.
    Asynchronous,
    /// Requests are answered inside `load`.
    Synchronous,
    /// Requests wait for `respond_to_all`.
    Manual,
}

/// What answers a request: its response, or `None` to answer it later.
type Answer = Arc<dyn Fn(&Resource) -> Option<Response> + Send + Sync>;

/// What a stub runs when it wants a [`poll`](StubFileSource::poll).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct Pending {
    resource: Resource,
    /// The answer for every kind when the request came, if there was one.
    answer: Option<Answer>,
    responder: Responder,
}

#[derive(Default)]
struct State {
    pending: VecMap<u64, Pending>,
    next_id: u64,
    responders: HashMap<Kind, Answer>,
    /// The answer for every kind; by default the per-kind ones.
    response: Option<Answer>,
    waker: Option<Waker>,
    /// The waker has asked for a poll that hasn't started yet.
    poll_asked: bool,
}

/// A file source for tests, which answers requests with the answers the
/// test sets, at once, at polls or when the test says.
pub struct StubFileSource {
    type_: ResponseType,
    state: Mutex<State>,
}

fn unexpected(resource: &Resource) -> Option<Response> {
    // C++ throws std::runtime_error("unexpected ... request").
    panic!("unexpected {:?} request: {}", resource.kind, resource.url)
}

impl StubFileSource {
    #[must_use]
    pub fn new(type_: ResponseType) -> Self {
        StubFileSource { type_, state: Mutex::default() }
    }

    /// A stub whose asynchronous requests wait for polls that run in the
    /// turns of `queue`.
    #[must_use]
    pub fn on_queue<S: 'static>(queue: &Queue<S>, type_: ResponseType) -> Arc<Self> {
        let stub = Arc::new(Self::new(type_));
        let (weak, queue) = (Arc::downgrade(&stub), queue.downgrade());
        stub.set_waker(Some(Arc::new(move || {
            let weak = Weak::clone(&weak);
            if let Some(queue) = queue.upgrade() {
                queue.post(move |_| {
                    if let Some(stub) = weak.upgrade() {
                        stub.poll();
                    }
                });
            }
        })));
        stub
    }

    /// Sets what an asynchronous stub runs, on any thread, when it wants the
    /// test to call [`poll`](Self::poll). It asks once until the poll
    /// starts.
    pub fn set_waker(&self, waker: Option<Waker>) {
        self.state.lock().waker = waker;
    }

    /// Sets the responder for one resource kind (`styleResponse`,
    /// `spriteJSONResponse`, `spriteImageResponse`, ...).
    pub fn set_response(&self, kind: Kind, responder: impl Fn(&Resource) -> Option<Response> + Send + Sync + 'static) {
        self.state.lock().responders.insert(kind, Arc::new(responder));
    }

    // gtests-core: assigning `response` in C++ (test/map/map.test.cpp
    // DisabledSources, test/map/prefetch.test.cpp).
    /// Replaces the responder for all kinds (C++ `fileSource->response = ...`).
    pub fn set_global_response(&self, responder: impl Fn(&Resource) -> Option<Response> + Send + Sync + 'static) {
        self.state.lock().response = Some(Arc::new(responder));
    }

    /// Answers `resource` with `answer`, or with the per-kind responder now.
    fn answer(&self, answer: Option<&Answer>, resource: &Resource) -> Option<Response> {
        let answer = match answer {
            Some(answer) => Arc::clone(answer),
            None => match self.state.lock().responders.get(&resource.kind).cloned() {
                Some(answer) => answer,
                None => return unexpected(resource),
            },
        };
        answer(resource)
    }

    /// Answers the requests pending now. Only for `ResponseType::Manual`.
    pub fn respond_to_all(&self) {
        assert_eq!(self.type_, ResponseType::Manual, "this method can only be called with ResponseType::Manual");
        let ids: Vec<u64> = self.state.lock().pending.keys().copied().collect();
        for id in ids {
            // A callback can cancel or start requests, so take each one out
            // of `pending` before calling it.
            let Some(pending) = self.state.lock().pending.remove(&id) else {
                continue;
            };
            if pending.responder.is_cancelled() {
                continue;
            }
            if let Some(res) = self.answer(pending.answer.as_ref(), &pending.resource) {
                pending.responder.send(res);
            }
        }
    }

    /// Answers the waiting requests of an asynchronous stub, on this thread.
    /// A request whose answer is an error, or `None`, waits for the next
    /// poll, which the waker asks for a millisecond later, from another
    /// thread.
    pub fn poll(&self) {
        let pending: Vec<(u64, Resource, Option<Answer>, Responder)> = {
            let mut state = self.state.lock();
            state.poll_asked = false;
            state.pending.retain(|_, pending| !pending.responder.is_cancelled());
            // A copy: an answer may start or cancel requests.
            state
                .pending
                .iter()
                .map(|(id, pending)| (*id, pending.resource.clone(), pending.answer.clone(), pending.responder.clone()))
                .collect()
        };
        for (id, resource, answer, responder) in pending {
            if responder.is_cancelled() {
                continue;
            }
            if let Some(res) = self.answer(answer.as_ref(), &resource) {
                // A request that failed is retried at the next poll.
                if res.error.is_none() {
                    self.state.lock().pending.remove(&id);
                }
                responder.send(res);
            }
        }
        let waker = {
            let mut state = self.state.lock();
            state.pending.retain(|_, pending| !pending.responder.is_cancelled());
            if state.pending.is_empty() || std::mem::replace(&mut state.poll_asked, true) {
                return;
            }
            state.waker.clone()
        };
        // The next timer tick, which leaves the thread that polls free
        // meanwhile.
        if let Some(waker) = waker {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1));
                waker();
            });
        }
    }

    fn ask_for_poll(&self) {
        let waker = {
            let mut state = self.state.lock();
            if std::mem::replace(&mut state.poll_asked, true) {
                return;
            }
            state.waker.clone()
        };
        if let Some(waker) = waker {
            waker();
        }
    }
}

impl FileSource for StubFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let (id, answer) = {
            let mut state = self.state.lock();
            state.next_id += 1;
            (state.next_id, state.response.clone())
        };
        match self.type_ {
            ResponseType::Synchronous => {
                if let Some(res) = self.answer(answer.as_ref(), resource) {
                    responder.send(res);
                }
            }
            ResponseType::Asynchronous | ResponseType::Manual => {
                self.state.lock().pending.insert(id, Pending { resource: resource.clone(), answer, responder });
                if self.type_ == ResponseType::Asynchronous {
                    self.ask_for_poll();
                }
            }
        }
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}
