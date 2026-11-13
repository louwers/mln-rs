// `RequestCoalescingMultithreaded` and `ThreadSafety` give a plain object a
// `Mailbox` on a tagged background scheduler; the port's mailboxes pin their
// target to one thread, so the worker is a pool `Actor` on the same
// `TaggedScheduler`, which is the same thing. The worker calls
// `AsyncTask::send` from pool threads through an `AsyncTaskSender` (C++: an
// `AsyncTask*`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};

use mln_util::identity::SimpleIdentity;
use parking_lot::Mutex;

use crate::actor::Actor;
use crate::scheduler::{self, SchedulerPtr, TaggedScheduler};
use crate::tests::gtest::{Failures, wait};
use crate::{AsyncTask, AsyncTaskSender, RunLoop};

struct TestWorker {
    async_: AsyncTaskSender,
}

impl TestWorker {
    fn run(&mut self) {
        for _ in 0..100_000 {
            self.async_.send();
        }
    }

    fn run_with_callback(&mut self, cb: impl FnOnce()) {
        for _ in 0..100_000 {
            self.async_.send();
        }
        cb();
    }

    fn sync(barrier: &mpsc::Sender<()>) {
        barrier.send(()).unwrap();
    }
}

// gtest: AsyncTask.RequestCoalescing
#[test]
fn request_coalescing() {
    let run_loop = RunLoop::new();

    let count = Rc::new(Cell::new(0u32));
    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(move || c.set(c.get() + 1));

    async_.send();
    async_.send();
    async_.send();
    async_.send();
    async_.send();

    run_loop.run_once();

    assert_eq!(count.get(), 1);
}

// gtest: AsyncTask.DestroyShouldNotRunQueue
#[test]
fn destroy_should_not_run_queue() {
    let _run_loop = RunLoop::new();
    let count = Rc::new(Cell::new(0u32));

    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(move || c.set(c.get() + 1));
    async_.send();
    drop(async_);

    assert_eq!(count.get(), 0);
}

// gtest: AsyncTask.DestroyAfterSignaling
#[test]
fn destroy_after_signaling() {
    let run_loop = RunLoop::new();

    // We're creating two tasks and signal both of them; the one that gets fired
    // first destroys the other one. Make sure that the second one we destroyed
    // doesn't fire.

    let task1: Rc<RefCell<Option<AsyncTask>>> = Rc::new(RefCell::new(None));
    let task2: Rc<RefCell<Option<AsyncTask>>> = Rc::new(RefCell::new(None));
    let failed = Rc::new(Cell::new(false));

    let (t1, t2, f) = (Rc::clone(&task1), Rc::clone(&task2), Rc::clone(&failed));
    *task1.borrow_mut() = Some(AsyncTask::new(move || {
        let other = t2.borrow_mut().take();
        drop(other);
        if t1.borrow().is_none() {
            f.set(true); // FAIL() << "Task was destroyed but invoked anyway";
        }
    }));
    let (t1, t2, f) = (Rc::clone(&task1), Rc::clone(&task2), Rc::clone(&failed));
    *task2.borrow_mut() = Some(AsyncTask::new(move || {
        let other = t1.borrow_mut().take();
        drop(other);
        if t2.borrow().is_none() {
            f.set(true); // FAIL() << "Task was destroyed but invoked anyway";
        }
    }));

    task1.borrow().as_ref().unwrap().send();
    task2.borrow().as_ref().unwrap().send();

    run_loop.run_once();

    assert!(!failed.get(), "Task was destroyed but invoked anyway");
}

// gtest: AsyncTask.RequestCoalescingMultithreaded
#[test]
fn request_coalescing_multithreaded() {
    let run_loop = RunLoop::new();

    let id = SimpleIdentity::new();
    let count = Rc::new(Cell::new(0u32));
    let num_threads = 25;
    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(move || c.set(c.get() + 1));

    let retainer = TaggedScheduler::new(scheduler::get_background(), id);
    let sender = async_.sender();
    let worker = Actor::new_tagged(&retainer, move |_| TestWorker { async_: sender });
    let worker_ref = worker.self_ref();

    for _ in 0..num_threads {
        worker_ref.invoke(TestWorker::run);
    }

    let (barrier, barrier_future) = mpsc::channel();
    worker_ref.invoke(move |_: &mut TestWorker| TestWorker::sync(&barrier));
    wait(&barrier_future);

    run_loop.run_once();

    assert_eq!(count.get(), 1);
}

// gtest: AsyncTask.ThreadSafety
#[test]
fn thread_safety() {
    let run_loop = RunLoop::new();

    let id = SimpleIdentity::new();
    let count = Rc::new(Cell::new(0u32));
    let num_threads = 25;
    let completed = Arc::new(AtomicU32::new(num_threads));

    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(move || c.set(c.get() + 1));

    let retainer = TaggedScheduler::new(scheduler::get_background(), id);
    let sender = async_.sender();
    let worker = Actor::new_tagged(&retainer, move |_| TestWorker { async_: sender });
    let worker_ref = worker.self_ref();

    for _ in 0..num_threads {
        // The callback runs on the worker, thus the atomic type.
        let completed = Arc::clone(&completed);
        let handle = run_loop.handle();
        worker_ref.invoke(move |w: &mut TestWorker| {
            w.run_with_callback(move || {
                if completed.fetch_sub(1, Ordering::SeqCst) == 1 {
                    handle.stop();
                }
            });
        });
    }

    run_loop.run();

    // We expect here more than 1 but 1 would also be
    // a valid result, although very unlikely (I hope).
    assert!(count.get() > 0);
}

// gtest: AsyncTask.scheduleAndReplyValue
#[test]
fn schedule_and_reply_value() {
    let run_loop = RunLoop::new();
    let failures = Failures::default();
    let caller_id = std::thread::current().id();
    let f = failures.clone();
    let run_in_background = move || -> i32 {
        f.expect(caller_id != std::thread::current().id(), "runs in the background");
        42
    };
    let f = failures.clone();
    let l = run_loop.clone();
    let on_result = move |res: i32| {
        f.expect(caller_id == std::thread::current().id(), "replies on the caller's thread");
        f.expect(42 == res, "42 == res");
        l.stop();
    };

    let scheduler: SchedulerPtr = scheduler::get_background();
    scheduler::schedule_and_reply_value(&*scheduler, SimpleIdentity::EMPTY, run_in_background, on_result);
    run_loop.run();
    failures.assert_none();
}

// gtest: AsyncTask.SequencedScheduler
#[test]
fn sequenced_scheduler() {
    let run_loop = RunLoop::new();
    let failures = Failures::default();
    let caller_id = std::thread::current().id();
    let bg_id = Arc::new(Mutex::new(None));
    let count = Arc::new(AtomicI32::new(0));

    let (f, b, c) = (failures.clone(), Arc::clone(&bg_id), Arc::clone(&count));
    let first = move || {
        f.expect(0 == c.load(Ordering::SeqCst), "0 == count");
        let id = std::thread::current().id();
        *b.lock() = Some(id);
        f.expect(caller_id != id, "caller_id != bg_id");
        c.fetch_add(1, Ordering::SeqCst);
    };
    let (f, b, c) = (failures.clone(), Arc::clone(&bg_id), Arc::clone(&count));
    let second = move || {
        f.expect(1 == c.load(Ordering::SeqCst), "1 == count");
        f.expect(*b.lock() == Some(std::thread::current().id()), "bg_id == this thread");
        c.fetch_add(1, Ordering::SeqCst);
    };
    let (f, b, c) = (failures.clone(), Arc::clone(&bg_id), Arc::clone(&count));
    let handle = run_loop.handle();
    let third = move || {
        f.expect(2 == c.load(Ordering::SeqCst), "2 == count");
        f.expect(*b.lock() == Some(std::thread::current().id()), "bg_id == this thread");
        handle.stop();
    };

    let scheduler = scheduler::get_sequenced();
    scheduler.schedule(Box::new(first));
    scheduler.schedule(Box::new(second));
    scheduler.schedule(Box::new(third));

    run_loop.run();
    failures.assert_none();
}

// gtest: AsyncTask.MultipleSequencedSchedulers
#[test]
fn multiple_sequenced_schedulers() {
    let schedulers_count = 10; // must match the value in the scheduler
    let mut schedulers: Vec<SchedulerPtr> = Vec::new();
    let same = |a: &SchedulerPtr, b: &SchedulerPtr| Arc::as_ptr(a).cast::<()>() == Arc::as_ptr(b).cast::<()>();

    // Regression check, the scheduler assignment was previously sensitive to the state of the weak references.
    // If expired weak references followed a still-valid one, both after the last-used index, the index would
    // be incremented multiple times.
    // (Rust also evaluates the right side first and drops the old value on
    // assignment.)
    #[allow(unused_assignments)]
    let mut temp = scheduler::get_sequenced();
    temp = scheduler::get_sequenced();
    let _temp = temp;

    // Check that exactly N unique schedulers are produced.
    // Note that this relies on no other threads requesting schedulers.
    for _ in 0..schedulers_count {
        let scheduler = scheduler::get_sequenced();
        assert!(!schedulers.iter().any(|s| same(s, &scheduler)));
        schedulers.push(scheduler);
    }
    assert!(same(&schedulers[0], &scheduler::get_sequenced()));
}
