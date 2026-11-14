use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::run_loop::{AsyncHandle, Core, RunLoop};

pub struct AsyncTask {
    handle: Rc<AsyncHandle>,
}

impl AsyncTask {
    /// Creates a task that runs `task` on the current thread's `RunLoop`.
    pub fn new(task: impl FnMut() + 'static) -> Self {
        let run_loop = RunLoop::get();
        let handle = Rc::new(AsyncHandle {
            pending: Arc::new(AtomicBool::new(false)),
            closed: Cell::new(false),
            task: RefCell::new(Box::new(task)),
            core: run_loop.core(),
        });
        run_loop.register(&handle);
        AsyncTask { handle }
    }

    /// Schedules the task. Calls before it runs coalesce into one run; a
    /// blocking `RunLoop::run` wakes up.
    pub fn send(&self) {
        self.handle.pending.store(true, Ordering::SeqCst);
        self.handle.core.async_pending.store(true, Ordering::SeqCst);
        self.handle.core.wake();
    }

    /// A `Send + Sync` handle whose `send()` is this task's `send()`: C++
    /// calls `AsyncTask::send` from other threads through a pointer (e.g.
    /// `NetworkStatus` observers).
    #[must_use]
    pub fn sender(&self) -> AsyncTaskSender {
        AsyncTaskSender { pending: Arc::clone(&self.handle.pending), core: Arc::clone(&self.handle.core) }
    }
}

impl Drop for AsyncTask {
    /// Marks the handle closed: the loop may still hold it while it runs, so
    /// the handle can outlive the task.
    fn drop(&mut self) {
        self.handle.closed.set(true);
    }
}

/// See `AsyncTask::sender`. Sending after the task is gone does nothing.
#[derive(Clone)]
pub struct AsyncTaskSender {
    pending: Arc<AtomicBool>,
    core: Arc<Core>,
}

impl AsyncTaskSender {
    pub fn send(&self) {
        self.pending.store(true, Ordering::SeqCst);
        self.core.async_pending.store(true, Ordering::SeqCst);
        self.core.wake();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn coalesces() {
        let run_loop = RunLoop::new();
        let count = Rc::new(Cell::new(0));
        let c = Rc::clone(&count);
        let task = AsyncTask::new(move || c.set(c.get() + 1));
        task.send();
        task.send();
        run_loop.run_once();
        assert_eq!(count.get(), 1);
        run_loop.run_once();
        assert_eq!(count.get(), 1);
        task.send();
        run_loop.run_once();
        assert_eq!(count.get(), 2);
        drop(task);
        run_loop.run_once();
    }
}
