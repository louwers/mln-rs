use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

use parking_lot::Mutex;

use crate::tests::gtest::{Failures, get, wait};
use crate::{ActorRef, RunLoop, Thread, Timer};

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
    let thread = Thread::new("Test", move |actor, _| TestObject::new(actor, tid, f));

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
    let thread = Thread::new("Test", move |actor, _| TestObject::new(actor, tid, f));

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
    fn new(_: ActorRef<TestWorker>, run_loop: &RunLoop) -> Self {
        TestWorker { timer: Timer::new(run_loop) }
    }

    fn send(cb: impl FnOnce()) {
        cb();
    }

    fn send_delayed(&self, cb: impl Fn() + 'static) {
        self.timer.start(Duration::from_millis(300), Duration::ZERO, cb);
    }
}

// gtest: Thread.ExecutesAfter
#[test]
fn executes_after() {
    let run_loop = RunLoop::new();
    let thread = Thread::new("Test", TestWorker::new);

    let did_work = Arc::new(AtomicBool::new(false));
    let did_after = Arc::new(AtomicBool::new(false));

    let w = Arc::clone(&did_work);
    thread.actor().invoke(move |_: &TestWorker| TestWorker::send(move || w.store(true, Ordering::SeqCst)));
    let a = Arc::clone(&did_after);
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

// gtest: Thread.ReferenceCanOutliveThread
#[test]
fn reference_can_outlive_thread() {
    let thread = Thread::new("Test", TestWorker::new);
    let worker = thread.actor();

    drop(thread);

    let happened = Arc::new(AtomicBool::new(false));
    for _ in 0..1000 {
        let h = Arc::clone(&happened);
        worker.invoke(move |_: &TestWorker| TestWorker::send(move || h.store(true, Ordering::SeqCst)));
    }

    std::thread::sleep(Duration::from_secs(10));
    assert!(!happened.load(Ordering::SeqCst), "Should never happen");
}

struct TestWorkerDelayedConstruction {
    _timer: Timer,
}

impl TestWorkerDelayedConstruction {
    fn new(_: ActorRef<TestWorkerDelayedConstruction>, run_loop: &RunLoop, start: &mpsc::Receiver<()>) -> Self {
        wait(start);
        TestWorkerDelayedConstruction { _timer: Timer::new(run_loop) }
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
    let thread =
        Thread::new("Test", move |actor, run_loop| TestWorkerDelayedConstruction::new(actor, run_loop, &start_future));

    let count = Arc::new(AtomicI32::new(0));

    for _ in 0..100 {
        let c = Arc::clone(&count);
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
    let thread =
        Thread::new("Test", move |actor, run_loop| TestWorkerDelayedConstruction::new(actor, run_loop, &start_future));

    let f = Arc::clone(&flag);
    thread.actor().invoke(move |_: &TestWorkerDelayedConstruction| {
        TestWorkerDelayedConstruction::send(move || f.store(true, Ordering::SeqCst));
    });
    let start = Mutex::new(Some(start));
    control.actor().invoke(move |o: &TestWorker| {
        o.send_delayed(move || {
            if let Some(start) = start.lock().take() {
                start.send(()).unwrap();
            }
        });
    });

    // Should not hang.
    drop(thread);

    // Should process the queue before destruction.
    assert!(flag.load(Ordering::SeqCst));
}
