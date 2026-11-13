// Port-only: a scheduler without threads, for a host that has one thread
// (the browser: a wasm32-unknown-unknown instance per page or Web Worker,
// each with its own memory). C++ has no counterpart; it always has threads.
//
// It keeps the tagged queues of `ThreadedScheduler` (thread_pool.rs), but no
// worker thread visits them: the host runs the tasks on its own thread with
// `run_until(deadline)`, in the same rounds of one task per tagged queue as
// a worker does, when its frame has time to spare. `wait_for_empty` runs the
// tag's tasks itself instead of waiting for a worker.
//
// A host installs one at startup with `scheduler::install_cooperative`,
// before any map or file source exists. From then on `GetBackground` and
// `GetSequenced` return it (one thread runs everything in order, which is
// as sequenced as it gets), and `util::Thread` keeps its object on the
// creating thread's RunLoop (thread.rs). Nothing changes for a host that
// doesn't install one.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_util::identity::SimpleIdentity;
use mln_util::time::Instant;
use parking_lot::Mutex;

use crate::scheduler::{Scheduler, Task};

/// A scheduler whose tasks the host runs on its own thread.
pub struct CooperativeScheduler {
    unique_id: SimpleIdentity,
    queues: Mutex<HashMap<SimpleIdentity, Arc<Mutex<VecDeque<Task>>>>>,
    task_count: AtomicUsize,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Default for CooperativeScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl CooperativeScheduler {
    #[must_use]
    pub fn new() -> Self {
        CooperativeScheduler {
            unique_id: SimpleIdentity::new(),
            queues: Mutex::new(HashMap::new()),
            task_count: AtomicUsize::new(0),
            waker: Mutex::new(None),
        }
    }

    /// Calls `waker`, from the scheduling thread, whenever a task arrives
    /// while none were waiting, so that the host knows to run them (as
    /// `RunLoop::set_waker`).
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.waker.lock() = waker;
    }

    /// Whether tasks are waiting.
    pub(crate) fn has_tasks(&self) -> bool {
        self.task_count.load(Ordering::SeqCst) != 0
    }

    /// Runs waiting tasks on the calling thread, in rounds of one task per
    /// tagged queue as a worker thread visits them, until none are left or
    /// `deadline` has passed (checked after each task). Returns whether
    /// tasks remain.
    pub fn run_until(&self, deadline: Option<Instant>) -> bool {
        while self.has_tasks() {
            // 1. Gather buckets for us to visit this iteration (dropping
            // the empty ones: a later task of the tag starts a new queue).
            let pending: Vec<Arc<Mutex<VecDeque<Task>>>> = {
                let mut queues = self.queues.lock();
                queues.retain(|_, queue| !queue.lock().is_empty());
                queues.values().cloned().collect()
            };
            if pending.is_empty() {
                break;
            }

            // 2. Visit a task from each
            for queue in pending {
                let Some(task) = queue.lock().pop_front() else {
                    continue;
                };
                self.run(task);
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return self.has_tasks();
                }
            }
        }
        self.has_tasks()
    }

    fn run(&self, task: Task) {
        self.task_count.fetch_sub(1, Ordering::SeqCst);
        task();
    }
}

impl Scheduler for CooperativeScheduler {
    fn schedule(&self, task: Task) {
        self.schedule_tagged(self.unique_id, task);
    }

    fn schedule_tagged(&self, tag: SimpleIdentity, task: Task) {
        let queue = Arc::clone(self.queues.lock().entry(tag).or_default());
        queue.lock().push_back(task);
        if self.task_count.fetch_add(1, Ordering::SeqCst) == 0 {
            let waker = self.waker.lock().clone();
            if let Some(waker) = waker {
                waker();
            }
        }
    }

    fn wait_for_empty(&self) {
        self.wait_for_empty_tagged(SimpleIdentity::EMPTY);
    }

    /// Runs the tag's tasks here, as there is no worker to wait for. Called
    /// from one of the tag's own tasks, it runs the ones queued after it but
    /// can't wait for itself (a `ThreadedScheduler` asserts there instead).
    fn wait_for_empty_tagged(&self, tag: SimpleIdentity) {
        let tag_to_find = if tag.is_empty() { self.unique_id } else { tag };
        let Some(queue) = self.queues.lock().get(&tag_to_find).cloned() else {
            return;
        };
        loop {
            let Some(task) = queue.lock().pop_front() else {
                break;
            };
            self.run(task);
        }

        // After waiting for the queue to empty, go ahead and erase it from the map.
        let mut queues = self.queues.lock();
        if queues.get(&tag_to_find).is_some_and(|q| Arc::ptr_eq(q, &queue)) {
            queues.remove(&tag_to_find);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(scheduler: &CooperativeScheduler, tag: SimpleIdentity, order: &Arc<Mutex<Vec<i32>>>, value: i32) {
        let order = Arc::clone(order);
        let main = std::thread::current().id();
        scheduler.schedule_tagged(
            tag,
            Box::new(move || {
                assert_eq!(std::thread::current().id(), main);
                order.lock().push(value);
            }),
        );
    }

    #[test]
    fn tasks_run_when_the_host_runs_them() {
        let scheduler = CooperativeScheduler::new();
        let (a, b) = (SimpleIdentity::new(), SimpleIdentity::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        for (tag, value) in [(a, 1), (a, 2), (b, 3)] {
            push(&scheduler, tag, &order, value);
        }
        assert!(order.lock().is_empty());
        assert!(scheduler.has_tasks());

        // A past deadline stops after the first task.
        assert!(scheduler.run_until(Some(Instant::now())));
        assert_eq!(order.lock().len(), 1);

        assert!(!scheduler.run_until(None));
        assert!(!scheduler.has_tasks());
        let order = order.lock().clone();
        // Each tag's tasks in order.
        let position = |value| order.iter().position(|&v| v == value).unwrap();
        assert!(position(1) < position(2));
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn a_round_visits_each_queue_once() {
        let scheduler = CooperativeScheduler::new();
        let (a, b) = (SimpleIdentity::new(), SimpleIdentity::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        for (tag, value) in [(a, 1), (a, 2), (a, 3), (b, 4)] {
            push(&scheduler, tag, &order, value);
        }
        scheduler.run_until(None);
        let order = order.lock().clone();
        // b's task runs in the first round, before a's last two.
        assert!(order.iter().position(|&v| v == 4).unwrap() < 2);
    }

    #[test]
    fn the_first_waiting_task_wakes_the_host() {
        let scheduler = CooperativeScheduler::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        scheduler.set_waker(Some(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })));
        let order = Arc::new(Mutex::new(Vec::new()));
        push(&scheduler, SimpleIdentity::new(), &order, 1);
        push(&scheduler, SimpleIdentity::new(), &order, 2);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        scheduler.run_until(None);
        push(&scheduler, SimpleIdentity::new(), &order, 3);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn wait_for_empty_runs_the_tag() {
        let scheduler = CooperativeScheduler::new();
        let (a, b) = (SimpleIdentity::new(), SimpleIdentity::new());
        let order = Arc::new(Mutex::new(Vec::new()));
        for (tag, value) in [(a, 1), (b, 2), (a, 3)] {
            push(&scheduler, tag, &order, value);
        }
        scheduler.wait_for_empty_tagged(a);
        assert_eq!(*order.lock(), vec![1, 3]);
        assert!(scheduler.has_tasks());
        scheduler.wait_for_empty_tagged(b);
        assert_eq!(*order.lock(), vec![1, 3, 2]);
        assert!(!scheduler.has_tasks());

        // The untagged queue.
        push(&scheduler, SimpleIdentity::new(), &order, 4);
        let untagged = Arc::clone(&order);
        scheduler.schedule(Box::new(move || untagged.lock().push(5)));
        scheduler.wait_for_empty();
        assert_eq!(*order.lock(), vec![1, 3, 2, 5]);
        assert!(scheduler.has_tasks());
    }

    #[test]
    fn wait_for_empty_from_a_task_of_the_tag() {
        let scheduler = Arc::new(CooperativeScheduler::new());
        let tag = SimpleIdentity::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let (inner, o) = (Arc::clone(&scheduler), Arc::clone(&order));
        scheduler.schedule_tagged(
            tag,
            Box::new(move || {
                o.lock().push(1);
                inner.wait_for_empty_tagged(tag);
                o.lock().push(3);
            }),
        );
        push(&scheduler, tag, &order, 2);
        assert!(!scheduler.run_until(None));
        assert_eq!(*order.lock(), vec![1, 2, 3]);
    }
}
