// Ports test/util/thread_local.test.cpp.
//
// C++ `util::ThreadLocal<T>` (a per-thread `T*` that does not own its value)
// is std's `thread_local!` in the port; these tests check the same
// properties on it: each thread sees its own value, an unset value is
// empty, and the storage does not take ownership.

use std::cell::RefCell;
use std::sync::{Arc, mpsc};

use mln_actor::{ActorRef, RunLoop, Thread};

use crate::get;

thread_local! {
    // `static ThreadLocal<int> number;` of TestThread
    static NUMBER: RefCell<Option<Arc<i32>>> = const { RefCell::new(None) };
    // `static ThreadLocal<int> data;` of TestThreadDataOwnership
    static DATA: RefCell<Option<Arc<i32>>> = const { RefCell::new(None) };
}

struct TestThread;

impl TestThread {
    fn new(_: ActorRef<TestThread>, number: Arc<i32>) -> Self {
        NUMBER.with(|n| *n.borrow_mut() = Some(number));
        TestThread
    }

    fn get_number(result: &mpsc::Sender<i32>) {
        result.send(NUMBER.with(|n| **n.borrow().as_ref().unwrap())).unwrap();
    }
}

impl Drop for TestThread {
    fn drop(&mut self) {
        NUMBER.with(|n| *n.borrow_mut() = None);
    }
}

// gtest: ThreadLocalStorage.Basic
#[test]
fn basic() {
    let _run_loop = RunLoop::new();

    let number1 = Arc::new(1);
    let number2 = Arc::new(2);
    let number3 = Arc::new(3);

    let (n1, n2, n3) = (number1.clone(), number2.clone(), number3.clone());
    let thread1 = Thread::new("Test", move |actor| TestThread::new(actor, n1));
    let thread2 = Thread::new("Test", move |actor| TestThread::new(actor, n2));
    let thread3 = Thread::new("Test", move |actor| TestThread::new(actor, n3));

    let thread1_ref = thread1.actor();
    let thread2_ref = thread2.actor();
    let thread3_ref = thread3.actor();

    let (result1, result1_future) = mpsc::channel();
    thread1_ref.invoke(move |_: &TestThread| TestThread::get_number(&result1));
    assert_eq!(*number1, get(&result1_future));

    let (result2, result2_future) = mpsc::channel();
    thread2_ref.invoke(move |_: &TestThread| TestThread::get_number(&result2));
    assert_eq!(*number2, get(&result2_future));

    let (result3, result3_future) = mpsc::channel();
    thread3_ref.invoke(move |_: &TestThread| TestThread::get_number(&result3));
    assert_eq!(*number3, get(&result3_future));
}

// gtest: ThreadLocalStorage.NotSetReturnsNull
#[test]
fn not_set_returns_null() {
    thread_local! {
        static NUMBER: RefCell<Option<Arc<i32>>> = const { RefCell::new(None) };
    }

    assert!(NUMBER.with(|n| n.borrow().is_none()));
}

struct TestThreadDataOwnership;

impl TestThreadDataOwnership {
    fn new(_: ActorRef<TestThreadDataOwnership>, data: Arc<i32>) -> Self {
        DATA.with(|d| *d.borrow_mut() = Some(data));
        TestThreadDataOwnership
    }
}

impl Drop for TestThreadDataOwnership {
    fn drop(&mut self) {
        DATA.with(|d| *d.borrow_mut() = None);
    }
}

// gtest: ThreadLocalStorage.ShouldNotTakeOwnership
#[test]
fn should_not_take_ownership() {
    let _run_loop = RunLoop::new();

    let data1 = Arc::new(10);
    let data2 = Arc::new(20);

    let (d1, d2) = (data1.clone(), data2.clone());
    let thread1 = Thread::new("Test", move |actor| TestThreadDataOwnership::new(actor, d1));
    let thread2 = Thread::new("Test", move |actor| TestThreadDataOwnership::new(actor, d2));

    drop(thread1);
    drop(thread2);

    // Will crash if ThreadLocal destroys
    // the pointer it is managing.
    assert_eq!(*data1, 10);
    assert_eq!(*data2, 20);
}
