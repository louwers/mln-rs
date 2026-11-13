// Ports test/util/thread.test.cpp.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

use mln_actor::actor::Actor;
use mln_actor::scheduler::{self, SchedulerPtr, TaggedScheduler, Task};
use mln_actor::{ActorRef, RunLoop, Thread, Timer};
use mln_util::identity::SimpleIdentity;

use crate::{Failures, get, wait};

struct TestObject {
    tid: ThreadId,
    failures: Failures,
}

impl TestObject {
    fn new(_: ActorRef<TestObject>, other_tid: ThreadId, failures: Failures) -> Self {
        let tid = std::thread::current().id();
        failures.expect(tid != other_tid, "Object is created on child thread");
        TestObject { tid, failures }
    }

    fn fn1(&self, val: i32) {
        self.failures.expect(self.tid == std::thread::current().id(), "fn1 on child thread");
        self.failures.expect(val == 1, "val == 1");
    }

    fn fn2(&self, cb: impl FnOnce(i32)) {
        self.failures.expect(self.tid == std::thread::current().id(), "fn2 on child thread");
        cb(1);
    }

    fn transfer_in(&self, val: i32) {
        self.failures.expect(self.tid == std::thread::current().id(), "transferIn on child thread");
        self.failures.expect(val == 1, "*val == 1");
    }

    fn transfer_in_shared(&self, val: i32) {
        self.failures.expect(self.tid == std::thread::current().id(), "transferInShared on child thread");
        self.failures.expect(val == 1, "*val == 1");
    }

    fn transfer_string(&self, string: &str) {
        self.failures.expect(self.tid == std::thread::current().id(), "transferString on child thread");
        self.failures.expect(string == "test", "string == \"test\"");
    }

    fn check_context(&self, result: &mpsc::Sender<bool>) {
        result.send(self.tid == std::thread::current().id()).unwrap();
    }

    fn sync(result: &mpsc::Sender<()>) {
        result.send(()).unwrap();
    }
}

impl Drop for TestObject {
    fn drop(&mut self) {
        // Object is destroyed on child thread
        self.failures.expect(self.tid == std::thread::current().id(), "Object is destroyed on child thread");
    }
}

// gtest: Thread.invoke
#[test]
fn invoke() {
    let tid = std::thread::current().id();
    let failures = Failures::default();

    let f = failures.clone();
    let thread = Thread::new("Test", move |actor| TestObject::new(actor, tid, f));

    thread.actor().invoke(|o: &TestObject| o.fn1(1));
    let f = failures.clone();
    thread.actor().invoke(move |o: &TestObject| o.fn2(move |result| f.expect(result == 1, "result == 1")));
    // The C++ test moves a `std::unique_ptr<int>` into the thread.
    let val = Box::new(1);
    thread.actor().invoke(move |o: &TestObject| o.transfer_in(*val));
    let shared = Arc::new(1);
    thread.actor().invoke(move |o: &TestObject| o.transfer_in_shared(*shared));

    let test = String::from("test");
    thread.actor().invoke(move |o: &TestObject| o.transfer_string(&test));

    // Make sure the message queue was consumed before ending the test.
    let (result, result_future) = mpsc::channel();
    thread.actor().invoke(move |_: &TestObject| TestObject::sync(&result));
    wait(&result_future);

    drop(thread);
    failures.assert_none();
}

// gtest: Thread.Context
#[test]
fn context() {
    let tid = std::thread::current().id();
    let failures = Failures::default();

    let f = failures.clone();
    let thread = Thread::new("Test", move |actor| TestObject::new(actor, tid, f));

    let (result, result_future) = mpsc::channel();
    thread.actor().invoke(move |o: &TestObject| o.check_context(&result));

    assert!(get(&result_future));

    drop(thread);
    failures.assert_none();
}

struct TestWorker {
    timer: Timer,
}

impl TestWorker {
    fn new(_: ActorRef<TestWorker>) -> Self {
        TestWorker { timer: Timer::new() }
    }

    fn send(cb: impl FnOnce()) {
        cb();
    }

    fn send_delayed(&self, cb: impl Fn() + 'static) {
        self.timer.start(Duration::from_millis(300), Duration::ZERO, cb);
    }
}

/// The pool actor flavour of `TestWorker`: a pool actor's object is `Send`,
/// and a `Timer` belongs to its thread's `RunLoop`, so it has no timer (only
/// `send` is used on pool workers).
struct PoolTestWorker;

impl PoolTestWorker {
    fn send(cb: impl FnOnce()) {
        cb();
    }
}

// gtest: Thread.ExecutesAfter
#[test]
fn executes_after() {
    let run_loop = RunLoop::new();
    let thread = Thread::new("Test", TestWorker::new);

    let did_work = Arc::new(AtomicBool::new(false));
    let did_after = Arc::new(AtomicBool::new(false));

    let w = did_work.clone();
    thread.actor().invoke(move |_: &TestWorker| TestWorker::send(move || w.store(true, Ordering::SeqCst)));
    let a = did_after.clone();
    let handle = run_loop.handle();
    thread.actor().invoke(move |_: &TestWorker| {
        TestWorker::send(move || {
            a.store(true, Ordering::SeqCst);
            handle.stop();
        });
    });

    run_loop.run();

    assert!(did_work.load(Ordering::SeqCst));
    assert!(did_after.load(Ordering::SeqCst));
}

// gtest: Thread.CanSelfWakeUp
#[test]
fn can_self_wake_up() {
    let run_loop = RunLoop::new();
    let thread = Thread::new("Test", TestWorker::new);

    let handle = run_loop.handle();
    thread.actor().invoke(move |o: &TestWorker| o.send_delayed(move || handle.stop()));

    run_loop.run();
}

// gtest: Thread.Concurrency
#[test]
fn concurrency() {
    let run_loop = RunLoop::new();

    let num_messages: usize = 100_000;
    let completed = Arc::new(AtomicUsize::new(num_messages));

    // TODO(port): platform::Settings and EXPERIMENTAL_THREAD_PRIORITY_WORKER
    // (thread priorities are not ported).

    let pool_worker = Actor::new(scheduler::get_background(), |_| PoolTestWorker);
    let pool_worker_ref = pool_worker.self_ref();

    let threaded_object = Thread::new("Test", TestWorker::new);
    let threaded_object_ref = threaded_object.actor();

    // 10 threads sending 100k messages to the Thread. The
    // idea here is to test if the scheduler is handling concurrency
    // correctly, otherwise this test should crash.
    for _ in 0..num_messages {
        let threaded_object_ref = threaded_object_ref.clone();
        let handle = run_loop.handle();
        let completed = completed.clone();
        pool_worker_ref.invoke(move |_: &mut PoolTestWorker| {
            PoolTestWorker::send(move || {
                threaded_object_ref.invoke(move |_: &TestWorker| {
                    TestWorker::send(move || {
                        if completed.fetch_sub(1, Ordering::SeqCst) == 1 {
                            handle.stop();
                        }
                    });
                });
            });
        });
    }

    run_loop.run();
}

// gtest: Thread.ThreadPoolMessaging
#[test]
fn thread_pool_messaging() {
    let run_loop = RunLoop::new();

    let pool_worker = Actor::new(scheduler::get_background(), |_| PoolTestWorker);
    let pool_worker_ref = pool_worker.self_ref();

    let threaded_object = Thread::new("Test", TestWorker::new);
    let threaded_object_ref = threaded_object.actor();

    // This is sending a message to the Thread from the main
    // thread. Then the Thread will send another message to
    // a worker on the ThreadPool.
    let p = pool_worker_ref.clone();
    let handle = run_loop.handle();
    threaded_object_ref.invoke(move |_: &TestWorker| {
        TestWorker::send(move || p.invoke(move |_: &mut PoolTestWorker| PoolTestWorker::send(move || handle.stop())));
    });

    run_loop.run();

    // Same as before, but in the opposite direction.
    let t = threaded_object_ref.clone();
    let handle = run_loop.handle();
    pool_worker_ref.invoke(move |_: &mut PoolTestWorker| {
        PoolTestWorker::send(move || t.invoke(move |_: &TestWorker| TestWorker::send(move || handle.stop())));
    });

    run_loop.run();
}

// gtest: Thread.ReferenceCanOutliveThread
#[test]
fn reference_can_outlive_thread() {
    let thread = Thread::new("Test", TestWorker::new);
    let worker = thread.actor();

    drop(thread);

    let happened = Arc::new(AtomicBool::new(false));
    for _ in 0..1000 {
        let h = happened.clone();
        worker.invoke(move |_: &TestWorker| TestWorker::send(move || h.store(true, Ordering::SeqCst)));
    }

    std::thread::sleep(std::time::Duration::from_secs(10));
    assert!(!happened.load(Ordering::SeqCst), "Should never happen");
}

struct TestWorkerDelayedConstruction {
    _timer: Timer,
}

impl TestWorkerDelayedConstruction {
    fn new(_: ActorRef<TestWorkerDelayedConstruction>, start: &mpsc::Receiver<()>) -> Self {
        wait(start);
        TestWorkerDelayedConstruction { _timer: Timer::new() }
    }

    fn send(cb: impl FnOnce()) {
        cb();
    }
}

// gtest: Thread.InvokeBeforeChildStarts
#[test]
fn invoke_before_child_starts() {
    let run_loop = RunLoop::new();

    let (start, start_future) = mpsc::channel();
    let thread = Thread::new("Test", move |actor| TestWorkerDelayedConstruction::new(actor, &start_future));

    let count = Arc::new(AtomicI32::new(0));

    for _ in 0..100 {
        let c = count.clone();
        thread.actor().invoke(move |_: &TestWorkerDelayedConstruction| {
            TestWorkerDelayedConstruction::send(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        });
    }

    let handle = run_loop.handle();
    thread
        .actor()
        .invoke(move |_: &TestWorkerDelayedConstruction| TestWorkerDelayedConstruction::send(move || handle.stop()));

    // This test will be flaky if messages are consumed before the target object is constructed.
    assert_eq!(count.load(Ordering::SeqCst), 0);

    start.send(()).unwrap();

    run_loop.run();

    assert_eq!(count.load(Ordering::SeqCst), 100);
}

// gtest: Thread.DeleteBeforeChildStarts
#[test]
fn delete_before_child_starts() {
    let flag = Arc::new(AtomicBool::new(false));
    let (start, start_future) = mpsc::channel();

    let control = Thread::new("Control", TestWorker::new);
    let thread = Thread::new("Test", move |actor| TestWorkerDelayedConstruction::new(actor, &start_future));

    let f = flag.clone();
    thread.actor().invoke(move |_: &TestWorkerDelayedConstruction| {
        TestWorkerDelayedConstruction::send(move || f.store(true, Ordering::SeqCst));
    });
    let start = Mutex::new(Some(start));
    control.actor().invoke(move |o: &TestWorker| {
        o.send_delayed(move || {
            if let Some(start) = start.lock().unwrap().take() {
                start.send(()).unwrap();
            }
        });
    });

    // Should not hang.
    drop(thread);

    // Should process the queue before destruction.
    assert!(flag.load(Ordering::SeqCst));
}

// gtest: Thread.PoolWait
#[test]
fn pool_wait() {
    let pool = scheduler::get_background();

    let thread_count = 10;
    for _ in 0..thread_count {
        pool.schedule(Box::new(|| std::thread::sleep(std::time::Duration::from_millis(100))));
    }

    pool.wait_for_empty();
}

// gtest: Thread.PoolWaitRecursiveAdd
#[test]
fn pool_wait_recursive_add() {
    let pool = scheduler::get_background();

    let p = pool.clone();
    pool.schedule(Box::new(move || {
        // Scheduled tasks can add more tasks
        let p2 = p.clone();
        p.schedule(Box::new(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            p2.schedule(Box::new(|| std::thread::sleep(std::time::Duration::from_millis(10))));
        }));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }));

    pool.wait_for_empty();
}

// gtest: Thread.PoolWaitAdd
#[test]
fn pool_wait_add() {
    #[derive(Default)]
    struct State {
        add_active: AtomicBool,
        added: AtomicI32,
        executed: AtomicI32,
    }

    let pool = scheduler::get_background();
    let seq = scheduler::get_sequenced();

    // add new tasks every few milliseconds
    let state = Arc::new(State::default());
    state.add_active.store(true, Ordering::SeqCst);

    {
        let pool = pool.clone();
        let state = state.clone();
        seq.schedule(Box::new(move || {
            while state.add_active.load(Ordering::SeqCst) {
                let s = state.clone();
                pool.schedule(Box::new(move || {
                    s.executed.fetch_add(1, Ordering::SeqCst);
                }));
                state.added.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    // Wait be sure some are added
    while state.added.load(Ordering::SeqCst) < 1 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Add an item that should take long enough to be confident that
    // more items would be added by the sequential task if not blocked
    pool.schedule(Box::new(|| std::thread::sleep(std::time::Duration::from_millis(100))));

    pool.wait_for_empty();

    state.add_active.store(false, Ordering::SeqCst);
    pool.wait_for_empty();

    assert!(state.added.load(Ordering::SeqCst) >= 1);
}

// gtest: Thread.WrongThread
#[test]
#[ignore = "C++ builds it only with NDEBUG: waitForEmpty from a pool thread asserts in debug builds; the port's ThreadedScheduler asserts in every build"]
fn wrong_thread() {
    let pool = scheduler::get_background();

    // Asserts in debug builds, silently ignored in release.
    let p = pool.clone();
    pool.schedule(Box::new(move || p.wait_for_empty()));

    pool.wait_for_empty();
}

fn make_counter_thread(pool: TaggedScheduler, stop_flag: Arc<AtomicBool>, counter: Arc<AtomicUsize>) -> Task {
    Box::new(move || {
        std::thread::sleep(std::time::Duration::from_micros(10));
        counter.fetch_add(1, Ordering::SeqCst);
        if !stop_flag.load(Ordering::SeqCst) {
            pool.schedule(make_counter_thread(pool.clone(), stop_flag.clone(), counter.clone()));
        }
    })
}

// gtest: Thread.TaggedPools
#[test]
fn tagged_pools() {
    let background: SchedulerPtr = scheduler::get_background();
    let pool_tag1 = TaggedScheduler::new(background.clone(), SimpleIdentity::new());
    let pool_tag2 = TaggedScheduler::new(background.clone(), SimpleIdentity::new());
    let stop_tasks1 = Arc::new(AtomicBool::new(false));
    let stop_tasks2 = Arc::new(AtomicBool::new(false));
    let run_count1 = Arc::new(AtomicUsize::new(0));
    let run_count2 = Arc::new(AtomicUsize::new(0));

    // Run a bunch of tasks on two different pool tags (that share the same thread pool)
    for _ in 0..50 {
        pool_tag1.schedule(make_counter_thread(pool_tag1.clone(), stop_tasks1.clone(), run_count1.clone()));
        pool_tag2.schedule(make_counter_thread(pool_tag2.clone(), stop_tasks2.clone(), run_count2.clone()));
    }
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Stop tasks on queue 1 and wait for it to empty
    stop_tasks1.store(true, Ordering::SeqCst);
    pool_tag1.wait_for_empty();
    let total_runs1 = run_count1.load(Ordering::SeqCst);

    // Do the same for queue 2
    stop_tasks2.store(true, Ordering::SeqCst);
    pool_tag2.wait_for_empty();
    let total_runs2 = run_count2.load(Ordering::SeqCst);

    // No other tasks in queue 1 ran while waiting on queue 2.
    assert_eq!(total_runs1, run_count1.load(Ordering::SeqCst));

    std::thread::sleep(std::time::Duration::from_millis(150));

    // Same for queue 2
    assert_eq!(total_runs2, run_count2.load(Ordering::SeqCst));
}
