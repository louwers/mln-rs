//! The work of a thread that owns some state: a runtime's core, or a render
//! session's worker. Any thread posts tasks; the owning thread runs them
//! with its state, between the turns of its loop.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(target_family = "wasm"))]
use mln_actor::RunLoop;
use parking_lot::Mutex;

type Task<S> = Box<dyn FnOnce(&mut S) + Send>;

/// A queue of tasks that take `&mut S`, which the thread that runs the queue
/// owns. Clones are handles to the same queue.
pub(crate) struct Queue<S> {
    inner: Arc<Inner<S>>,
}

struct Inner<S> {
    tasks: Mutex<VecDeque<Task<S>>>,
    /// Told when a task arrives, so the thread that runs the queue wakes.
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    stopped: AtomicBool,
}

impl<S> Clone for Queue<S> {
    fn clone(&self) -> Self {
        Queue { inner: Arc::clone(&self.inner) }
    }
}

impl<S> Default for Queue<S> {
    fn default() -> Self {
        Queue {
            inner: Arc::new(Inner {
                tasks: Mutex::new(VecDeque::new()),
                waker: Mutex::new(None),
                stopped: AtomicBool::new(false),
            }),
        }
    }
}

impl<S: 'static> Queue<S> {
    /// Runs `task` on the queue's thread, after the tasks posted before it.
    pub(crate) fn post(&self, task: impl FnOnce(&mut S) + Send + 'static) {
        self.inner.tasks.lock().push_back(Box::new(task));
        self.wake();
    }

    /// Calls `waker`, from the posting thread, whenever a task arrives.
    pub(crate) fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.inner.waker.lock() = waker;
    }

    /// Wakes the queue's thread, as a task would.
    pub(crate) fn wake(&self) {
        let waker = self.inner.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Runs the tasks waiting now and those they post, with `state`, and
    /// returns whether any ran.
    pub(crate) fn run_once(&self, state: &mut S) -> bool {
        let mut ran = false;
        loop {
            let Some(task) = self.inner.tasks.lock().pop_front() else { return ran };
            task(state);
            ran = true;
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.inner.tasks.lock().is_empty()
    }

    /// Tells the queue's thread that it can stop running the queue, once the
    /// tasks posted before this have run.
    pub(crate) fn stop(&self) {
        let inner = Arc::clone(&self.inner);
        self.post(move |_| inner.stopped.store(true, Ordering::SeqCst));
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Runs the queue with `state`, and `run_loop`'s other work, on this
    /// thread until the queue stops: what a thread of its own does.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn run_until_stopped(&self, run_loop: &RunLoop, state: &mut S) {
        let handle = run_loop.handle();
        self.set_waker(Some(Arc::new(move || handle.wake())));
        loop {
            run_loop.run_once();
            self.run_once(state);
            if self.is_stopped() {
                return;
            }
            if !run_loop.has_pending() && !self.has_pending() {
                run_loop.wait_for_work();
            }
        }
    }
}
