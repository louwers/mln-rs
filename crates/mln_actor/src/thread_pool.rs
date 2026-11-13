use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

use mln_util::identity::SimpleIdentity;
use parking_lot::{Condvar, Mutex};

use crate::scheduler::{Scheduler, Task};

#[derive(Default)]
struct Queue {
    /// running tasks
    running_count: AtomicUsize,
    /// queue empty condition
    cv: Condvar,
    /// pending task queue (the C++ `lock` guards it)
    queue: Mutex<VecDeque<Task>>,
}

struct Shared {
    unique_id: SimpleIdentity,
    cv_available: Condvar,
    worker_mutex: Mutex<bool>, // terminated
    tagged_queue: Mutex<HashMap<SimpleIdentity, Arc<Queue>>>,
    task_count: AtomicUsize,
}

thread_local! {
    static OWNING_THREAD_POOL: Cell<usize> = const { Cell::new(0) };
}

pub(crate) struct ThreadedScheduler {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl ThreadedScheduler {
    #[must_use]
    pub(crate) fn new(n: usize) -> Self {
        let shared = Arc::new(Shared {
            unique_id: SimpleIdentity::new(),
            cv_available: Condvar::new(),
            worker_mutex: Mutex::new(false),
            tagged_queue: Mutex::new(HashMap::new()),
            task_count: AtomicUsize::new(0),
        });
        let threads = (0..n).map(|index| make_scheduler_thread(Arc::clone(&shared), index)).collect();
        ThreadedScheduler { shared, threads }
    }

    fn this_thread_is_owned(&self) -> bool {
        OWNING_THREAD_POOL.with(Cell::get) == Arc::as_ptr(&self.shared) as usize
    }
}

fn make_scheduler_thread(shared: Arc<Shared>, index: usize) -> JoinHandle<()> {
    mln_util::thread::spawn(format!("Worker {}", index + 1), move || {
        OWNING_THREAD_POOL.with(|p| p.set(Arc::as_ptr(&shared) as usize));
        loop {
            {
                let mut terminated = shared.worker_mutex.lock();
                if !*terminated && shared.task_count.load(Ordering::SeqCst) == 0 {
                    shared.cv_available.wait(&mut terminated);
                }
                if *terminated {
                    break;
                }
                // Let other threads run
            }

            // 1. Gather buckets for us to visit this iteration
            let pending: Vec<Arc<Queue>> = shared.tagged_queue.lock().values().cloned().collect();

            // 2. Visit a task from each
            for q in pending {
                let tasklet = {
                    let mut queue = q.queue.lock();
                    let tasklet = queue.pop_front();
                    if tasklet.is_some() {
                        q.running_count.fetch_add(1, Ordering::SeqCst);
                    }
                    tasklet
                };
                let Some(tasklet) = tasklet else {
                    continue;
                };

                shared.task_count.fetch_sub(1, Ordering::SeqCst);

                // A task that panics still counts as done, so that
                // `waitForEmpty` returns before the panic ends this
                // worker.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(tasklet));

                // The function and its captures are gone before
                // unblocking `waitForEmpty`.
                if q.running_count.fetch_sub(1, Ordering::SeqCst) == 1 {
                    let queue = q.queue.lock();
                    if queue.is_empty() {
                        q.cv.notify_all();
                    }
                }
                if let Err(payload) = result {
                    std::panic::resume_unwind(payload);
                }
            }
        }
    })
    .expect("failed to spawn worker thread")
}

impl Scheduler for ThreadedScheduler {
    fn schedule(&self, task: Task) {
        self.schedule_tagged(self.shared.unique_id, task);
    }

    fn schedule_tagged(&self, tag: SimpleIdentity, task: Task) {
        let q = {
            let mut tagged_queue = self.shared.tagged_queue.lock();
            // find or insert
            Arc::clone(tagged_queue.entry(tag).or_default())
        };

        {
            let mut queue = q.queue.lock();
            queue.push_back(task);
            self.shared.task_count.fetch_add(1, Ordering::SeqCst);
        }

        // Take the worker lock before notifying to prevent threads from
        // waiting while we try to wake them
        let _worker_lock = self.shared.worker_mutex.lock();
        self.shared.cv_available.notify_one();
    }

    fn wait_for_empty(&self) {
        self.wait_for_empty_tagged(SimpleIdentity::EMPTY);
    }

    fn wait_for_empty_tagged(&self, tag: SimpleIdentity) {
        // Must not be called from a thread in our pool, or we would deadlock
        assert!(!self.this_thread_is_owned());
        if self.this_thread_is_owned() {
            return;
        }
        let tag_to_find = if tag.is_empty() { self.shared.unique_id } else { tag };
        let q = {
            let tagged_queue = self.shared.tagged_queue.lock();
            match tagged_queue.get(&tag_to_find) {
                Some(q) => Arc::clone(q),
                None => return,
            }
        };

        let mut queue = q.queue.lock();
        while queue.len() + q.running_count.load(Ordering::SeqCst) != 0 {
            q.cv.wait(&mut queue);
        }
        drop(queue);

        // After waiting for the queue to empty, go ahead and erase it from the map.
        self.shared.tagged_queue.lock().remove(&tag_to_find);
    }
}

impl Drop for ThreadedScheduler {
    fn drop(&mut self) {
        assert!(!self.this_thread_is_owned());
        // terminate(): wake up all threads so that they shut down
        *self.shared.worker_mutex.lock() = true;
        self.shared.cv_available.notify_all();
        for thread in self.threads.drain(..) {
            mln_util::thread::join(thread);
        }
    }
}

/// A scheduler with four worker threads.
pub(crate) struct ThreadPool {
    scheduler: ThreadedScheduler,
}

impl Default for ThreadPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadPool {
    #[must_use]
    pub(crate) fn new() -> Self {
        ThreadPool { scheduler: ThreadedScheduler::new(1 + 3) }
    }
}

impl Scheduler for ThreadPool {
    fn schedule(&self, task: Task) {
        self.scheduler.schedule(task);
    }
    fn schedule_tagged(&self, tag: SimpleIdentity, task: Task) {
        self.scheduler.schedule_tagged(tag, task);
    }
    fn wait_for_empty(&self) {
        self.scheduler.wait_for_empty();
    }
    fn wait_for_empty_tagged(&self, tag: SimpleIdentity) {
        self.scheduler.wait_for_empty_tagged(tag);
    }
}

/// A scheduler with one worker thread, so its tasks run in order.
pub(crate) struct SequencedScheduler {
    scheduler: ThreadedScheduler,
}

impl Default for SequencedScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl SequencedScheduler {
    #[must_use]
    pub(crate) fn new() -> Self {
        SequencedScheduler { scheduler: ThreadedScheduler::new(1) }
    }
}

impl Scheduler for SequencedScheduler {
    fn schedule(&self, task: Task) {
        self.scheduler.schedule(task);
    }
    fn schedule_tagged(&self, tag: SimpleIdentity, task: Task) {
        self.scheduler.schedule_tagged(tag, task);
    }
    fn wait_for_empty(&self) {
        self.scheduler.wait_for_empty();
    }
    fn wait_for_empty_tagged(&self, tag: SimpleIdentity) {
        self.scheduler.wait_for_empty_tagged(tag);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::run_loop::RunLoop;
    use crate::scheduler::schedule_and_reply_value;

    #[test]
    fn runs_tagged_tasks_and_waits() {
        let pool = ThreadPool::new();
        let tag = SimpleIdentity::new();
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..50 {
            let c = Arc::clone(&count);
            pool.schedule_tagged(
                tag,
                Box::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }
        pool.wait_for_empty_tagged(tag);
        assert_eq!(count.load(Ordering::SeqCst), 50);
    }

    #[test]
    fn reply_runs_on_the_calling_loop() {
        let run_loop = RunLoop::new();
        let pool = ThreadPool::new();
        let result = Rc::new(RefCell::new(None));
        let r = Rc::clone(&result);
        let main = std::thread::current().id();
        schedule_and_reply_value(
            &pool,
            SimpleIdentity::new(),
            move || {
                assert_ne!(std::thread::current().id(), main);
                6 * 7
            },
            move |value: i32| {
                assert_eq!(std::thread::current().id(), main);
                *r.borrow_mut() = Some(value);
            },
        );
        while result.borrow().is_none() {
            run_loop.run_once();
            std::thread::yield_now();
        }
        assert_eq!(*result.borrow(), Some(42));
    }
}
