// The asynchronous mode polls the pending requests from an AsyncTask on the
// stub's RunLoop, re-armed every millisecond while requests are pending.
// The stub lives on the thread that created it; requests must come from that
// thread, and it answers them there. A cancelled request leaves `pending` at
// the next poll.

use std::cell::RefCell;
use std::rc::Rc;

use mln_actor::{AsyncTask, LocalObject, RunLoop};
use mln_util::hash::HashMap;
use mln_vec_map::VecMap;

use crate::file_source::FileSource;
use crate::request::Responder;
use crate::resource::{Kind, Resource};
use crate::response::Response;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseType {
    Asynchronous,
    Synchronous,
    /// Requests wait for `respond_to_all`.
    Manual,
}

/// What answers a request: its response, or `None` to answer it later.
pub(crate) type Answer = Rc<dyn Fn(&Resource) -> Option<Response>>;

#[derive(Default)]
struct State {
    pending: VecMap<u64, (Resource, Answer, Responder)>,
    next_id: u64,
    timer: Option<AsyncTask>,
    responders: HashMap<Kind, Answer>,
    /// The answer for every kind; by default the per-kind ones.
    response: Option<Answer>,
}

pub struct StubFileSource {
    type_: ResponseType,
    state: LocalObject<RefCell<State>>,
}

fn unexpected(resource: &Resource) -> Option<Response> {
    // C++ throws std::runtime_error("unexpected ... request").
    panic!("unexpected {:?} request: {}", resource.kind, resource.url)
}

impl StubFileSource {
    /// A stub on `run_loop`, which polls the asynchronous one's requests.
    #[must_use]
    pub fn new(run_loop: &RunLoop, type_: ResponseType) -> Self {
        let state = Rc::new(RefCell::new(State::default()));
        if type_ == ResponseType::Asynchronous {
            let weak = Rc::downgrade(&state);
            let timer = AsyncTask::new(run_loop, move || {
                if let Some(state) = weak.upgrade() {
                    Self::poll(&state);
                }
            });
            state.borrow_mut().timer = Some(timer);
        }
        StubFileSource { type_, state: LocalObject::from_rc(state) }
    }

    fn state(&self) -> Rc<RefCell<State>> {
        self.state.get().expect("StubFileSource used from another thread")
    }

    /// Sets the responder for one resource kind (`styleResponse`,
    /// `spriteJSONResponse`, `spriteImageResponse`, ...).
    pub fn set_response(&self, kind: Kind, responder: impl Fn(&Resource) -> Option<Response> + 'static) {
        self.state().borrow_mut().responders.insert(kind, Rc::new(responder));
    }

    // gtests-core: assigning `response` in C++ (test/map/map.test.cpp
    // DisabledSources, test/map/prefetch.test.cpp).
    /// Replaces the responder for all kinds (C++ `fileSource->response = ...`).
    pub fn set_global_response(&self, responder: impl Fn(&Resource) -> Option<Response> + 'static) {
        self.state().borrow_mut().response = Some(Rc::new(responder));
    }

    /// The `response` member a request captures: the global responder if
    /// one was set, else `defaultResponse`, which dispatches to the per-kind
    /// responder current when it is called (so a responder replaced while a
    /// request is pending, e.g. after an error response, answers the retry).
    fn response(&self) -> Answer {
        let state = self.state();
        if let Some(response) = state.borrow().response.clone() {
            return response;
        }
        let weak = Rc::downgrade(&state);
        Rc::new(move |resource: &Resource| {
            let responder = weak.upgrade().and_then(|state| state.borrow().responders.get(&resource.kind).cloned());
            match responder {
                Some(responder) => responder(resource),
                None => unexpected(resource),
            }
        })
    }

    /// Answers the requests pending now. Only for `ResponseType::Manual`.
    pub fn respond_to_all(&self) {
        assert_eq!(self.type_, ResponseType::Manual, "this method can only be called with ResponseType::Manual");
        let state = self.state();
        let ids: Vec<u64> = state.borrow().pending.keys().copied().collect();
        for id in ids {
            // A callback can cancel or start requests, so take each one out
            // of `pending` before calling it.
            let Some((resource, answer, responder)) = state.borrow_mut().pending.remove(&id) else {
                continue;
            };
            if responder.is_cancelled() {
                continue;
            }
            if let Some(res) = answer(&resource) {
                responder.send(res);
            }
        }
    }

    fn schedule_poll(state: &Rc<RefCell<State>>) {
        if let Some(timer) = &state.borrow().timer {
            timer.send();
        }
    }

    /// The timer callback.
    fn poll(state: &Rc<RefCell<State>>) {
        state.borrow_mut().pending.retain(|_, (_, _, responder)| !responder.is_cancelled());
        // A copy: an answer may start or cancel requests.
        let pending: Vec<(u64, Resource, Answer, Responder)> = state
            .borrow()
            .pending
            .iter()
            .map(|(id, (resource, answer, responder))| (*id, resource.clone(), Rc::clone(answer), responder.clone()))
            .collect();
        for (id, resource, answer, responder) in pending {
            if responder.is_cancelled() {
                continue;
            }
            if let Some(res) = answer(&resource) {
                // A request that failed is retried at the next poll.
                if res.error.is_none() {
                    state.borrow_mut().pending.remove(&id);
                }
                responder.send(res);
            }
        }
        state.borrow_mut().pending.retain(|_, (_, _, responder)| !responder.is_cancelled());
        if !state.borrow().pending.is_empty() {
            // The next timer tick.
            std::thread::sleep(std::time::Duration::from_millis(1));
            Self::schedule_poll(state);
        }
    }
}

impl FileSource for StubFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let state = self.state();
        let id = {
            let mut s = state.borrow_mut();
            s.next_id += 1;
            s.next_id
        };
        let answer = self.response();
        match self.type_ {
            ResponseType::Synchronous => {
                if let Some(res) = answer(resource) {
                    responder.send(res);
                }
            }
            ResponseType::Asynchronous | ResponseType::Manual => {
                state.borrow_mut().pending.insert(id, (resource.clone(), answer, responder));
                Self::schedule_poll(&state);
            }
        }
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}
