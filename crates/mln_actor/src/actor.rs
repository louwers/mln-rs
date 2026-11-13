// Ports include/mln/actor/actor.hpp for objects that may run on any thread
// of a scheduler (a ThreadPool), with the mailbox semantics of
// src/mln/actor/mailbox.cpp. Added by `sources` for GeometryTileWorker.
//
// `mailbox.rs` pins its target to the thread that opened it (the object is
// not `Send`). A pool actor's object is `Send` and lives behind a mutex:
// `receive()` runs one message with the object locked, which is the C++
// guarantee (messages run one at a time, in order; the receiving mutex),
// on whichever pool thread picks the task up. Messages are closures over
// `&mut O` (C++: a member function pointer plus arguments).
//
// `close()` (from `Drop`) waits for a running message, as the C++
// receiving mutex does, and drops the object afterwards. `abandon()` stops
// pushes without waiting.
//
// gtests-core: closing from inside one of the actor's own messages (C++:
// the recursive receiving mutex lets an actor destroy itself, see
// test/actor/actor.test.cpp DestructionAllowedInReceiveOnSameThread) does
// not wait: it closes the queue, and `receive()` drops the object once that
// message returns (C++ destroys it during the message, which Rust's `&mut O`
// borrow rules out).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak, mpsc};
use std::thread::ThreadId;

use mln_util::identity::SimpleIdentity;
use parking_lot::Mutex;

use crate::scheduler::{SchedulerPtr, TaggedScheduler, WeakScheduler};

/// A message to a pool actor's object.
pub type PoolMessage<O> = Box<dyn FnOnce(&mut O) + Send + 'static>;

struct Queue<O> {
    messages: VecDeque<PoolMessage<O>>,
    closed: bool,
}

/// The `Mailbox` of a pool actor, with its target object.
pub struct PoolMailbox<O: Send + 'static> {
    /// The receiving mutex; `None` once closed.
    object: Mutex<Option<O>>,
    /// The pushing mutex and the queue mutex.
    queue: Mutex<Queue<O>>,
    abandoned: AtomicBool,
    /// gtests-core: the thread running a message, for `close()` from inside
    /// a message.
    receiving_thread: Mutex<Option<ThreadId>>,
    scheduler: WeakScheduler,
    scheduler_tag: Option<SimpleIdentity>,
    weak_self: Weak<PoolMailbox<O>>,
}

impl<O: Send + 'static> PoolMailbox<O> {
    /// `push(std::unique_ptr<Message>)`.
    fn push(&self, message: PoolMessage<O>) {
        if self.abandoned.load(Ordering::SeqCst) {
            return;
        }
        let was_empty = {
            let mut queue = self.queue.lock();
            if queue.closed {
                return;
            }
            let was_empty = queue.messages.is_empty();
            queue.messages.push_back(message);
            was_empty
        };
        if was_empty {
            self.schedule_to_receive();
        }
    }

    /// `receive()`: runs the first message, and schedules the next one.
    fn receive(&self) {
        let mut object = self.object.lock();
        let Some(target) = object.as_mut() else {
            return; // closed
        };

        let (message, was_empty) = {
            let mut queue = self.queue.lock();
            if queue.closed {
                return;
            }
            let Some(message) = queue.messages.pop_front() else {
                return;
            };
            (message, queue.messages.is_empty())
        };

        *self.receiving_thread.lock() = Some(std::thread::current().id());
        message(target);
        *self.receiving_thread.lock() = None;
        // gtests-core: the message closed the mailbox (its actor was dropped
        // from inside it): drop the object now, outside the lock.
        if self.queue.lock().closed {
            let closed_object = object.take();
            drop(object);
            drop(closed_object);
            return;
        }
        drop(object);

        // If there are more messages in the queue and the scheduler is still
        // active, create a new task to handle the next one.
        if !was_empty {
            self.schedule_to_receive();
        }
    }

    fn schedule_to_receive(&self) {
        let Some(scheduler) = self.scheduler.upgrade() else {
            return;
        };
        let mailbox = self.weak_self.clone();
        let task = Box::new(move || {
            if let Some(mailbox) = mailbox.upgrade() {
                mailbox.receive();
            }
        });
        match self.scheduler_tag {
            Some(tag) => scheduler.schedule_tagged(tag, task),
            None => scheduler.schedule(task),
        }
    }

    /// `abandon()`: drops pushes from now on.
    pub fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }

    /// `close()`: blocks until no message is running; no message runs
    /// afterwards. Returns the object.
    fn close(&self) -> Option<O> {
        self.abandon();
        if *self.receiving_thread.lock() == Some(std::thread::current().id()) {
            // gtests-core: inside one of our own messages; `receive()` drops
            // the object when it returns.
            let mut queue = self.queue.lock();
            queue.closed = true;
            queue.messages.clear();
            return None;
        }
        let mut object = self.object.lock();
        let mut queue = self.queue.lock();
        queue.closed = true;
        queue.messages.clear();
        drop(queue);
        object.take()
    }
}

/// `ActorRef<O>` to a pool actor. Sending to a reference whose actor is gone
/// does nothing.
pub struct PoolActorRef<O: Send + 'static> {
    mailbox: Weak<PoolMailbox<O>>,
}

impl<O: Send + 'static> Clone for PoolActorRef<O> {
    fn clone(&self) -> Self {
        PoolActorRef { mailbox: self.mailbox.clone() }
    }
}

impl<O: Send + 'static> PoolActorRef<O> {
    /// `invoke(fn, args...)`.
    pub fn invoke(&self, f: impl FnOnce(&mut O) + Send + 'static) {
        if let Some(mailbox) = self.mailbox.upgrade() {
            mailbox.push(Box::new(f));
        }
    }

    /// `ask(fn, args...)` (gtests-core): the returned receiver is the C++
    /// `std::future`. If the actor is gone (or closes before the message
    /// runs), the message is dropped and `recv()` fails, as `get()` throws
    /// `broken_promise` in C++.
    pub fn ask<R: Send + 'static>(&self, f: impl FnOnce(&mut O) -> R + Send + 'static) -> mpsc::Receiver<R> {
        let (promise, future) = mpsc::sync_channel(1);
        self.invoke(move |object: &mut O| {
            let _ = promise.send(f(object));
        });
        future
    }
}

/// `Actor<O>`: owns the object; messages run on `scheduler`'s threads.
pub struct Actor<O: Send + 'static> {
    /// `retainer`: keeps the scheduler alive.
    _retainer: SchedulerPtr,
    mailbox: Arc<PoolMailbox<O>>,
}

impl<O: Send + 'static> Actor<O> {
    /// `Actor(const TaggedScheduler&, args...)`: `make` gets the actor's own
    /// reference, as an `O(ActorRef<O>, ...)` constructor does.
    pub fn new_tagged(scheduler: &TaggedScheduler, make: impl FnOnce(PoolActorRef<O>) -> O) -> Self {
        Self::create(scheduler.get().clone(), Some(scheduler.tag), make)
    }

    /// `Actor(std::shared_ptr<Scheduler>, args...)`.
    pub fn new(scheduler: SchedulerPtr, make: impl FnOnce(PoolActorRef<O>) -> O) -> Self {
        Self::create(scheduler, None, make)
    }

    fn create(scheduler: SchedulerPtr, tag: Option<SimpleIdentity>, make: impl FnOnce(PoolActorRef<O>) -> O) -> Self {
        let mailbox = Arc::new_cyclic(|weak_self: &Weak<PoolMailbox<O>>| PoolMailbox {
            object: Mutex::new(None),
            queue: Mutex::new(Queue { messages: VecDeque::new(), closed: false }),
            abandoned: AtomicBool::new(false),
            receiving_thread: Mutex::new(None),
            scheduler: Arc::downgrade(&scheduler),
            scheduler_tag: tag,
            weak_self: weak_self.clone(),
        });
        let object = make(PoolActorRef { mailbox: Arc::downgrade(&mailbox) });
        *mailbox.object.lock() = Some(object);
        // Messages sent from `make` were queued before the object existed.
        let pending = !mailbox.queue.lock().messages.is_empty();
        if pending {
            mailbox.schedule_to_receive();
        }
        Actor { _retainer: scheduler, mailbox }
    }

    /// `self()`.
    #[must_use]
    pub fn self_ref(&self) -> PoolActorRef<O> {
        PoolActorRef { mailbox: Arc::downgrade(&self.mailbox) }
    }

    /// The mailbox's `abandon()`.
    pub fn abandon(&self) {
        self.mailbox.abandon();
    }
}

impl<O: Send + 'static> Drop for Actor<O> {
    fn drop(&mut self) {
        let object = self.mailbox.close();
        drop(object);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::thread_pool::ThreadPool;

    struct Counter {
        count: u32,
        done: mpsc::Sender<u32>,
    }

    #[test]
    fn messages_run_in_order_on_the_pool() {
        let pool: SchedulerPtr = Arc::new(ThreadPool::new());
        let (tx, rx) = mpsc::channel();
        let actor = Actor::new(pool.clone(), |_self_ref| Counter { count: 0, done: tx });
        for i in 0..100u32 {
            actor.self_ref().invoke(move |c: &mut Counter| {
                assert_eq!(c.count, i);
                c.count += 1;
                if c.count == 100 {
                    c.done.send(c.count).unwrap();
                }
            });
        }
        assert_eq!(rx.recv().unwrap(), 100);
        drop(actor);
    }

    #[test]
    fn self_sends_and_drop_waits() {
        struct Ping {
            self_ref: PoolActorRef<Ping>,
            left: u32,
            done: mpsc::Sender<()>,
        }
        fn ping(p: &mut Ping) {
            if p.left == 0 {
                p.done.send(()).unwrap();
                return;
            }
            p.left -= 1;
            p.self_ref.invoke(ping);
        }
        let pool: SchedulerPtr = Arc::new(ThreadPool::new());
        let (tx, rx) = mpsc::channel();
        let actor = Actor::new(pool, |self_ref| Ping { self_ref, left: 10, done: tx });
        actor.self_ref().invoke(ping);
        rx.recv().unwrap();
        let weak = actor.self_ref();
        drop(actor);
        // Sending to a dead actor does nothing.
        weak.invoke(|_p: &mut Ping| panic!("must not run"));
    }
}
