use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use parking_lot::Mutex;

use crate::actor::{Actor, PoolActorRef};
use crate::scheduler::{self, Scheduler, SchedulerPtr, Task};
use crate::tests::gtest::{get, wait, wait_for};

// gtest: Actor.Construction
#[test]
fn construction() {
    struct TestActor;
    let mut constructed = false;
    let _test = Actor::new(scheduler::get_background(), |_self: PoolActorRef<TestActor>| {
        constructed = true;
        TestActor
    });
    assert!(constructed);
}

// gtest: Actor.Destruction
#[test]
fn destruction() {
    struct TestActor {
        destructed: Arc<AtomicBool>,
    }
    impl Drop for TestActor {
        fn drop(&mut self) {
            self.destructed.store(true, Ordering::SeqCst);
        }
    }

    let destructed = Arc::new(AtomicBool::new(false));
    {
        let d = destructed.clone();
        let _test = Actor::new(scheduler::get_background(), move |_| TestActor { destructed: d });
    }
    assert!(destructed.load(Ordering::SeqCst));
}

// gtest: Actor.DestructionBlocksOnReceive
#[test]
fn destruction_blocks_on_receive() {
    // Destruction blocks until the actor is not receiving.
    struct TestActor {
        promise: mpsc::Sender<()>,
        future: mpsc::Receiver<()>,
        waited: AtomicBool,
    }
    impl TestActor {
        fn wait(&mut self) {
            self.promise.send(()).unwrap();
            wait(&self.future);
            std::thread::sleep(Duration::from_millis(1));
            self.waited.store(true, Ordering::SeqCst);
        }
    }
    impl Drop for TestActor {
        fn drop(&mut self) {
            assert!(self.waited.load(Ordering::SeqCst));
        }
    }

    let (entered_promise, entered_future) = mpsc::channel();
    let (exiting_promise, exiting_future) = mpsc::channel();

    let test = Actor::new(scheduler::get_background(), move |_| TestActor {
        promise: entered_promise,
        future: exiting_future,
        waited: AtomicBool::new(false),
    });

    test.self_ref().invoke(TestActor::wait);
    wait(&entered_future);
    exiting_promise.send(()).unwrap();
    drop(test);
}

// gtest: Actor.DestructionBlocksOnSend
#[test]
fn destruction_blocks_on_send() {
    // Destruction blocks until the actor is not being sent a message.
    struct TestScheduler {
        promise: Mutex<mpsc::Sender<()>>,
        future: Mutex<mpsc::Receiver<()>>,
        waited: AtomicBool,
    }
    impl Drop for TestScheduler {
        fn drop(&mut self) {
            assert!(self.waited.load(Ordering::SeqCst));
        }
    }
    impl Scheduler for TestScheduler {
        fn wait_for_empty(&self) {
            unreachable!();
        }
        fn schedule(&self, _task: Task) {
            self.promise.lock().send(()).unwrap();
            wait(&self.future.lock());
            std::thread::sleep(Duration::from_millis(1));
            self.waited.store(true, Ordering::SeqCst);
        }
    }

    struct TestActor;

    let (entered_promise, entered_future) = mpsc::channel();
    let (exiting_promise, exiting_future) = mpsc::channel();

    // C++ `Actor(Scheduler&)` does not retain the scheduler; the port's
    // `Actor` always does (an `Arc`), so the scheduler goes away when its
    // last user (here: the sending thread) lets go of it.
    let scheduler: SchedulerPtr = Arc::new(TestScheduler {
        promise: Mutex::new(entered_promise),
        future: Mutex::new(exiting_future),
        waited: AtomicBool::new(false),
    });
    let actor = Actor::new(scheduler.clone(), |_| TestActor);

    let actor_ref = actor.self_ref();
    let thread = std::thread::spawn(move || actor_ref.invoke(|_: &mut TestActor| {}));

    wait(&entered_future);
    exiting_promise.send(()).unwrap();

    drop(actor);
    drop(scheduler);
    thread.join().expect("~TestScheduler: waited");
}

// gtest: Actor.DestructionAllowedInReceiveOnSameThread
#[test]
fn destruction_allowed_in_receive_on_same_thread() {
    // Destruction doesn't block if occurring on the same
    // thread as receive(). This prevents deadlocks and
    // allows for self-closing actors
    struct TestActor;
    impl TestActor {
        fn call_me_back(callback: impl FnOnce()) {
            callback();
        }
    }

    let (callback_fired_promise, callback_fired_future) = mpsc::channel();

    let retainer: SchedulerPtr = scheduler::get_background();
    let test = Arc::new(Mutex::new(Some(Actor::new(retainer.clone(), |_| TestActor))));

    // Callback (triggered while mutex is locked in Mailbox::receive())
    let slot = test.clone();
    let self_ref = test.lock().as_ref().unwrap().self_ref();
    self_ref.invoke(move |_: &mut TestActor| {
        TestActor::call_me_back(|| {
            // Destroy the Actor/Mailbox in the same thread
            let actor = slot.lock().take();
            drop(actor);
            callback_fired_promise.send(()).unwrap();
        });
    });

    let status = wait_for(&callback_fired_future, Duration::from_secs(1));
    assert!(status.is_some());
}

// gtest: Actor.SelfDestructionDoesntCrashWaitingReceivingThreads
#[test]
fn self_destruction_doesnt_crash_waiting_receiving_threads() {
    // Ensures destruction doesn't cause waiting threads to
    // crash when a actor closes it's own mailbox from a
    // callback
    struct TestActor;
    impl TestActor {
        fn call_me_back(callback: impl FnOnce()) {
            callback();
        }
    }

    let (actor_closed_promise, actor_closed_future) = mpsc::channel();

    let closing_actor = Arc::new(Mutex::new(Some(Actor::new(scheduler::get_background(), |_| TestActor))));
    let waiting_actor = Actor::new(scheduler::get_background(), |_| TestActor);

    let waiting_message_processed = Arc::new(AtomicBool::new(false));

    let closing_ref = closing_actor.lock().as_ref().unwrap().self_ref();
    let waiting_ref = waiting_actor.self_ref();
    let slot = closing_actor.clone();
    let processed = waiting_message_processed.clone();
    let queued_ok = Arc::new(AtomicBool::new(false));
    let queued_ok_in = queued_ok.clone();

    // Callback (triggered while mutex is locked in Mailbox::receive())
    closing_ref.clone().invoke(move |_: &mut TestActor| {
        TestActor::call_me_back(move || {
            // Queue up another message from another thread
            let (message_queued_promise, message_queued_future) = mpsc::channel();
            waiting_ref.invoke(move |_: &mut TestActor| {
                TestActor::call_me_back(move || {
                    // This will be waiting on the mutex in
                    // Mailbox::receive(), holding a lock on the weak_ptr so
                    // the mailbox is not destroyed
                    closing_ref.invoke(move |_: &mut TestActor| {
                        TestActor::call_me_back(|| processed.store(true, Ordering::SeqCst));
                    });
                    message_queued_promise.send(()).unwrap();
                });
            });

            // Wait for the message to be queued
            queued_ok_in.store(wait_for(&message_queued_future, Duration::from_secs(1)).is_some(), Ordering::SeqCst);

            // Destroy the Actor/Mailbox in the same thread
            let actor = slot.lock().take();
            drop(actor);
            actor_closed_promise.send(()).unwrap();
        });
    });

    let status = wait_for(&actor_closed_future, Duration::from_secs(1));
    assert!(status.is_some());
    assert!(queued_ok.load(Ordering::SeqCst));
    assert!(!waiting_message_processed.load(Ordering::SeqCst));
    drop(waiting_actor);
}

// gtest: Actor.OrderedMailbox
#[test]
fn ordered_mailbox() {
    // Messages are processed in order.
    struct TestActor {
        last: i32,
        promise: mpsc::Sender<bool>,
        in_order: bool,
    }
    impl TestActor {
        fn receive(&mut self, i: i32) {
            self.in_order &= i == self.last + 1;
            self.last = i;
        }
        fn end(&mut self) {
            self.promise.send(self.in_order).unwrap();
        }
    }

    let (ended_promise, ended_future) = mpsc::channel();
    let test =
        Actor::new(scheduler::get_background(), move |_| TestActor { last: 0, promise: ended_promise, in_order: true });

    for i in 1..=10 {
        test.self_ref().invoke(move |a: &mut TestActor| a.receive(i));
    }

    test.self_ref().invoke(TestActor::end);
    assert!(get(&ended_future), "EXPECT_EQ(i, last + 1)");
}

// gtest: Actor.NonConcurrentMailbox
#[test]
fn non_concurrent_mailbox() {
    // An individual actor is never itself concurrent.
    struct TestActor {
        last: i32,
        promise: mpsc::Sender<bool>,
        in_order: bool,
    }
    impl TestActor {
        fn receive(&mut self, i: i32) {
            self.in_order &= i == self.last + 1;
            self.last = i;
            std::thread::sleep(Duration::from_millis(1));
        }
        fn end(&mut self) {
            self.promise.send(self.in_order).unwrap();
        }
    }

    let (ended_promise, ended_future) = mpsc::channel();
    let test =
        Actor::new(scheduler::get_background(), move |_| TestActor { last: 0, promise: ended_promise, in_order: true });

    for i in 1..=10 {
        test.self_ref().invoke(move |a: &mut TestActor| a.receive(i));
    }

    test.self_ref().invoke(TestActor::end);
    assert!(get(&ended_future), "EXPECT_EQ(i, last + 1)");
}

// gtest: Actor.Ask
#[test]
fn ask() {
    // Asking for a result
    struct TestActor;
    impl TestActor {
        fn double_it(i: i32) -> i32 {
            i * 2
        }
    }

    let test = Actor::new(scheduler::get_background(), |_| TestActor);

    let result = test.self_ref().ask(|_: &mut TestActor| TestActor::double_it(1));
    let status = wait_for(&result, Duration::from_secs(1));
    assert_eq!(Some(2), status);
}

// gtest: Actor.AskVoid
#[test]
fn ask_void() {
    // Ask waits for void methods
    struct TestActor {
        executed: Arc<AtomicBool>,
    }
    impl TestActor {
        fn do_it(&mut self) {
            self.executed.store(true, Ordering::SeqCst);
        }
    }

    let executed = Arc::new(AtomicBool::new(false));
    let e = executed.clone();
    let actor = Actor::new(scheduler::get_background(), move |_| TestActor { executed: e });

    wait(&actor.self_ref().ask(TestActor::do_it));
    assert!(executed.load(Ordering::SeqCst));
}

// gtest: Actor.NoSelfActorRef
#[test]
fn no_self_actor_ref() {
    // Not all actors need a reference to self

    // Trivially constructable
    struct Trivial;

    // With arguments
    struct WithArguments {
        promise: mpsc::Sender<()>,
    }
    impl WithArguments {
        fn receive(&mut self) {
            self.promise.send(()).unwrap();
        }
    }

    let _trivial = Actor::new(scheduler::get_background(), |_| Trivial);

    let (promise, future) = mpsc::channel();
    let with_arguments = Actor::new(scheduler::get_background(), move |_| WithArguments { promise });
    with_arguments.self_ref().invoke(WithArguments::receive);
    wait(&future);
}
