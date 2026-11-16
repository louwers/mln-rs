use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{AsyncTask, RunLoop};

// gtest: AsyncTask.RequestCoalescing
#[test]
fn request_coalescing() {
    let run_loop = RunLoop::new();

    let count = Rc::new(Cell::new(0u32));
    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(&run_loop, move || c.set(c.get() + 1));

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
    let run_loop = RunLoop::new();
    let count = Rc::new(Cell::new(0u32));

    let c = Rc::clone(&count);
    let async_ = AsyncTask::new(&run_loop, move || c.set(c.get() + 1));
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
    *task1.borrow_mut() = Some(AsyncTask::new(&run_loop, move || {
        let other = t2.borrow_mut().take();
        drop(other);
        if t1.borrow().is_none() {
            f.set(true); // FAIL() << "Task was destroyed but invoked anyway";
        }
    }));
    let (t1, t2, f) = (Rc::clone(&task1), Rc::clone(&task2), Rc::clone(&failed));
    *task2.borrow_mut() = Some(AsyncTask::new(&run_loop, move || {
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
