// Ports include/mln/actor/mailbox.hpp, src/mln/actor/mailbox.cpp and
// include/mln/actor/message.hpp.
//
// A C++ message holds a reference to its target object; the mailbox runs
// it on the scheduler's thread. Here the target lives in the registry of the
// thread that opened the mailbox (see local_object.rs) and a message is a
// closure over the target. So a mailbox's scheduler must run its tasks on
// that thread (a RunLoop; actors on thread pools are a TODO(port)).
//
// Locking: C++ uses a pushing mutex, a recursive receiving mutex, a queue
// mutex and an Idle/Processing/Abandoned state to keep `close()` from
// racing a `receive()` on another thread. With the target pinned to one
// thread, `receive()` and `close()` both run there, so one mutex over the
// queue and the closed flag gives the same guarantees: after `close()` no
// message runs, and messages pushed to a closed mailbox are dropped.

use std::any::Any;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex, Weak};

use mln_util::identity::SimpleIdentity;
use mln_util::sync::MutexExt;

use crate::local_object;
use crate::scheduler::{Scheduler, WeakScheduler};

/// `actor::Message`: a call on the mailbox's target object.
pub type Message = Box<dyn FnOnce(&dyn Any) + Send + 'static>;

struct State {
    queue: VecDeque<Message>,
    closed: bool,
    scheduler: Option<WeakScheduler>,
    scheduler_tag: Option<SimpleIdentity>,
}

/// `Mailbox`.
pub struct Mailbox {
    state: Mutex<State>,
    /// The registry key of the target object on the receiving thread.
    id: u64,
    weak_self: Weak<Mailbox>,
}

impl Mailbox {
    /// `Mailbox()`: not open yet; messages queue until `open`.
    #[must_use]
    pub fn new() -> Arc<Mailbox> {
        Arc::new_cyclic(|weak_self| Mailbox {
            state: Mutex::new(State { queue: VecDeque::new(), closed: false, scheduler: None, scheduler_tag: None }),
            id: local_object::next_id(),
            weak_self: weak_self.clone(),
        })
    }

    /// `Mailbox(Scheduler&)` for `object`, which lives on this thread: the
    /// scheduler must run its tasks on this thread too.
    pub fn with_target(scheduler: &Arc<dyn Scheduler + Send + Sync>, object: Rc<dyn Any>) -> Arc<Mailbox> {
        let mailbox = Mailbox::new();
        mailbox.open(scheduler, object);
        mailbox
    }

    /// `open(Scheduler&)`, binding `object` (on the calling thread) as the
    /// target of the messages.
    pub fn open(&self, scheduler: &Arc<dyn Scheduler + Send + Sync>, object: Rc<dyn Any>) {
        let mut state = self.state.lock_unpoisoned();
        assert!(state.scheduler.is_none());
        if state.closed {
            return;
        }
        local_object::insert(self.id, object);
        state.scheduler = Some(Arc::downgrade(scheduler));
        if !state.queue.is_empty() {
            self.schedule_to_receive(&state, None);
        }
    }

    /// `open(const TaggedScheduler&)`.
    pub fn open_tagged(&self, scheduler: &Arc<dyn Scheduler + Send + Sync>, tag: SimpleIdentity, object: Rc<dyn Any>) {
        self.state.lock_unpoisoned().scheduler_tag = Some(tag);
        self.open(scheduler, object);
    }

    /// `close()`: no message runs after this returns. Call it on the target's
    /// thread (it unregisters the target there).
    pub fn close(&self) {
        let object = {
            let mut state = self.state.lock_unpoisoned();
            state.closed = true;
            state.scheduler = None;
            local_object::remove(self.id)
        };
        // The target is dropped (if this was its last owner) outside the
        // lock: its destructor may push to or close other mailboxes.
        drop(object);
    }

    /// `isOpen()`.
    pub fn is_open(&self) -> bool {
        let state = self.state.lock_unpoisoned();
        !state.closed && state.scheduler.as_ref().is_some_and(|s| s.strong_count() > 0)
    }

    /// `push(std::unique_ptr<Message>)`.
    pub fn push(&self, message: Message) {
        let mut state = self.state.lock_unpoisoned();
        if state.closed {
            return;
        }
        let was_empty = state.queue.is_empty();
        state.queue.push_back(message);
        if was_empty {
            let tag = state.scheduler_tag;
            self.schedule_to_receive(&state, tag);
        }
    }

    /// `receive()`: runs the first message, and schedules the next one.
    pub fn receive(&self) {
        let (message, was_empty) = {
            let mut state = self.state.lock_unpoisoned();
            if state.closed {
                return;
            }
            let Some(message) = state.queue.pop_front() else {
                return;
            };
            (message, state.queue.is_empty())
        };

        // The target stays alive while its message runs, even if the message
        // closes the mailbox (C++: the recursive receiving mutex).
        if let Some(object) = local_object::get(self.id) {
            message(&*object);
        }

        // If there are more messages in the queue and the scheduler is still
        // active, create a new task to handle the next one.
        if !was_empty {
            let state = self.state.lock_unpoisoned();
            if !state.closed {
                self.schedule_to_receive(&state, None);
            }
        }
    }

    fn schedule_to_receive(&self, state: &State, tag: Option<SimpleIdentity>) {
        let Some(scheduler) = state.scheduler.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let mailbox = self.weak_self.clone();
        let set_to_receive = Box::new(move || {
            if let Some(locked) = mailbox.upgrade() {
                locked.receive();
            }
        });
        match tag {
            Some(tag) => scheduler.schedule_tagged(tag, set_to_receive),
            None => scheduler.schedule(set_to_receive),
        }
    }
}
