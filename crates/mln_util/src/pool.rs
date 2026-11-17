//! Where background work runs: a [`Pool`] of worker threads, and the
//! [`TaskGroup`]s of its tasks that an owner waits for.
//!
//! A pool's tasks queue in lanes: the pool's own, and one per group. Its
//! workers take one task from each lane with work in turn, so a lane with
//! many tasks queued doesn't hold back the tasks of the others.
//!
//! A pool without threads ([`Pool::local`]) is for a host with one thread,
//! the browser: its tasks run when that thread calls [`Pool::run_until`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::thread::ThreadId;

use parking_lot::{Condvar, Mutex};

#[cfg(not(target_family = "wasm"))]
use crate::logging::{Event, Log};
#[cfg(not(target_family = "wasm"))]
use crate::thread::Joined;
use crate::time::Instant;

/// A task that runs on a pool.
pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// What a pool calls when work arrives while it had none.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Where a job hands its result: called at most once, from any thread,
/// while the job runs or later. Dropping it uncalled drops the result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

/// Runs background work on worker threads, or, without threads, on the
/// thread that runs it. Cheap to clone: clones share the pool. A worker
/// starts when a task arrives while every running worker is busy, up to the
/// pool's thread count, so a pool that runs nothing starts no threads. Its
/// threads end once the last clone and the last [`TaskGroup`] of it drop,
/// dropping the tasks still queued.
#[derive(Clone)]
pub struct Pool {
    workers: Arc<Workers>,
}

/// A lane of tasks of one pool, which an owner waits for with
/// [`wait`](Self::wait). Cheap to clone: clones share the lane. Dropping it
/// leaves its queued tasks to run.
#[derive(Clone)]
pub struct TaskGroup {
    pool: Pool,
    lane: Arc<Lane>,
}

/// The pool's threads, which stop when this drops.
struct Workers {
    shared: Arc<Shared>,
}

struct Shared {
    ring: Mutex<Ring>,
    /// Signalled when a lane joins the ring, or the pool stops.
    work: Condvar,
    /// The pool's own lane, for [`Pool::spawn`].
    lane: Arc<Lane>,
    waker: Mutex<Option<Waker>>,
    /// The most worker threads the pool starts: none for a local pool.
    max_threads: usize,
    /// The workers started so far.
    threads: Mutex<Threads>,
}

#[derive(Default)]
struct Threads {
    #[cfg(not(target_family = "wasm"))]
    joined: Vec<Joined>,
    ids: Vec<ThreadId>,
}

/// The lanes with queued tasks, each once, in the order the workers visit
/// them.
#[derive(Default)]
struct Ring {
    lanes: VecDeque<Arc<Lane>>,
    /// The workers started, or starting.
    started: usize,
    /// The workers waiting for a task.
    idle: usize,
    stopped: bool,
}

#[derive(Default)]
struct Lane {
    state: Mutex<LaneState>,
    /// Signalled when one of the lane's tasks ends.
    ended: Condvar,
}

#[derive(Default)]
struct LaneState {
    tasks: VecDeque<Task>,
    running: usize,
    /// Whether the lane is in the ring, or a worker that took it from the
    /// ring decides whether it goes back.
    queued: bool,
}

impl Pool {
    /// A pool of up to `threads` worker threads, which start as tasks
    /// arrive.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn new(threads: usize) -> Pool {
        Pool { workers: Arc::new(Workers { shared: Arc::new(Shared::new(threads)) }) }
    }

    /// A pool without threads, whose tasks run when [`run_until`](Self::run_until)
    /// runs them, on one thread.
    #[must_use]
    pub fn local() -> Pool {
        Pool { workers: Arc::new(Workers { shared: Arc::new(Shared::new(0)) }) }
    }

    /// The most worker threads the pool runs: none for a
    /// [`local`](Self::local) pool.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.workers.shared.max_threads
    }

    /// Runs `task` on the pool, in its own lane.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) {
        self.workers.shared.push(&self.workers.shared.lane, Box::new(task));
    }

    /// Calls `waker`, from the spawning thread, whenever a task arrives
    /// while none were queued, so that the host of a local pool knows to
    /// run it.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.workers.shared.waker.lock() = waker;
    }

    /// Runs queued tasks on this thread, one from each lane in turn, until
    /// none are left or `deadline` has passed (checked after each task).
    /// Returns whether tasks remain.
    #[must_use = "tasks may remain once the deadline passed"]
    pub fn run_until(&self, deadline: Option<Instant>) -> bool {
        let shared = &self.workers.shared;
        loop {
            let next = shared.next(&mut shared.ring.lock());
            let Some((lane, task)) = next else { return false };
            lane.run(task);
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return !shared.ring.lock().lanes.is_empty();
            }
        }
    }

    /// Runs queued tasks on this thread until none are left, including
    /// those they spawn.
    pub fn run_all(&self) {
        let _ = self.run_until(None);
    }

    /// Whether this thread is one of the pool's workers.
    fn is_worker(&self) -> bool {
        self.workers.shared.threads.lock().ids.contains(&std::thread::current().id())
    }
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool").field("threads", &self.threads()).finish_non_exhaustive()
    }
}

impl TaskGroup {
    /// A new lane of `pool`.
    #[must_use]
    pub fn new(pool: &Pool) -> TaskGroup {
        TaskGroup { pool: pool.clone(), lane: Arc::default() }
    }

    /// The pool the group's tasks run on.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Runs `task` on the pool, in the group's lane.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) {
        self.pool.workers.shared.push(&self.lane, Box::new(task));
    }

    /// Returns once none of the group's tasks are queued or running,
    /// including those they spawn into the group meanwhile. It runs the
    /// queued ones on this thread, where a task that panics counts as ended
    /// as it does on a worker, then waits for those that run on the pool's
    /// workers. On a [`local`](Pool::local) pool it doesn't wait: a
    /// task that runs there runs on this thread, below this call.
    ///
    /// # Panics
    ///
    /// On one of the pool's workers, which a running task could be waiting
    /// for.
    pub fn wait(&self) {
        assert!(!self.pool.is_worker(), "a task group was waited on from a task of its pool");
        let waits = self.pool.threads() > 0;
        let lane = &*self.lane;
        let mut state = lane.state.lock();
        loop {
            if let Some(task) = state.tasks.pop_front() {
                state.running += 1;
                drop(state);
                // As on a worker, a task that panics ends as any other.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lane.run(task)));
                state = lane.state.lock();
            } else if state.running == 0 || !waits {
                return;
            } else {
                lane.ended.wait(&mut state);
            }
        }
    }
}

impl std::fmt::Debug for TaskGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskGroup").field("pool", &self.pool).finish_non_exhaustive()
    }
}

impl Shared {
    fn new(max_threads: usize) -> Shared {
        Shared {
            ring: Mutex::default(),
            work: Condvar::new(),
            lane: Arc::default(),
            waker: Mutex::new(None),
            max_threads,
            threads: Mutex::default(),
        }
    }

    fn push(self: &Arc<Self>, lane: &Arc<Lane>, task: Task) {
        let joins = {
            let mut state = lane.state.lock();
            state.tasks.push_back(task);
            !std::mem::replace(&mut state.queued, true)
        };
        let (was_idle, start) = {
            let mut ring = self.ring.lock();
            let was_idle = joins && ring.lanes.is_empty();
            if joins {
                ring.lanes.push_back(Arc::clone(lane));
                self.work.notify_one();
            }
            // Every worker is busy: another starts, if the pool may run one.
            let start = ring.idle == 0 && !ring.stopped && ring.started < self.max_threads;
            ring.started += usize::from(start);
            (was_idle, start)
        };
        if start {
            self.start_worker();
        }
        if was_idle {
            let waker = self.waker.lock().clone();
            if let Some(waker) = waker {
                waker();
            }
        }
    }

    /// Takes the next task, from the lane at the front of the ring, which
    /// goes to the back if it has more. Its lane counts it running.
    fn next(&self, ring: &mut Ring) -> Option<(Arc<Lane>, Task)> {
        while let Some(lane) = ring.lanes.pop_front() {
            let mut state = lane.state.lock();
            let Some(task) = state.tasks.pop_front() else {
                state.queued = false;
                continue;
            };
            state.running += 1;
            if state.tasks.is_empty() {
                state.queued = false;
            } else {
                ring.lanes.push_back(Arc::clone(&lane));
                // Another worker takes the lane's next task.
                self.work.notify_one();
            }
            drop(state);
            return Some((lane, task));
        }
        None
    }

    /// Starts the worker that `push` counted started.
    #[cfg_attr(target_family = "wasm", expect(clippy::unused_self, reason = "the browser's pools start no threads"))]
    fn start_worker(self: &Arc<Self>) {
        #[cfg(not(target_family = "wasm"))]
        {
            let mut threads = self.threads.lock();
            let shared = Arc::clone(self);
            match crate::thread::spawn(format!("Worker {}", threads.ids.len() + 1), move || shared.work()) {
                Ok(thread) => {
                    threads.ids.push(thread.thread().id());
                    threads.joined.push(Joined::new(thread));
                }
                // The running workers take the tasks; without any, they
                // wait for a later task to start one.
                Err(error) => {
                    self.ring.lock().started -= 1;
                    Log::error(Event::General, format!("can't start a pool worker: {error}"));
                }
            }
        }
    }

    /// A worker's loop, until the pool stops.
    #[cfg(not(target_family = "wasm"))]
    fn work(&self) {
        loop {
            let next = {
                let mut ring = self.ring.lock();
                loop {
                    if ring.stopped {
                        return;
                    }
                    if let Some(next) = self.next(&mut ring) {
                        break next;
                    }
                    ring.idle += 1;
                    self.work.wait(&mut ring);
                    ring.idle -= 1;
                }
            };
            let (lane, task) = next;
            // A task that panics ends as any other: the panic hook reported
            // it, and the worker goes on.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lane.run(task)));
        }
    }
}

impl Lane {
    /// Runs `task`, which the lane counts running, and counts it ended once
    /// it and its captures are gone, even if it panics.
    fn run(&self, task: Task) {
        struct Ended<'a>(&'a Lane);
        impl Drop for Ended<'_> {
            fn drop(&mut self) {
                self.0.state.lock().running -= 1;
                self.0.ended.notify_all();
            }
        }
        let _ended = Ended(self);
        task();
    }
}

impl Drop for Workers {
    /// Stops the workers once their tasks return, and waits for them unless
    /// it drops on a library thread (`thread::Joined`), such as one of them.
    fn drop(&mut self) {
        self.shared.ring.lock().stopped = true;
        self.shared.work.notify_all();
        // Joined outside the lock, which a task that spawns takes.
        let threads = std::mem::take(&mut *self.shared.threads.lock());
        drop(threads);
        // The tasks left, dropped here rather than with the last worker.
        let lanes = std::mem::take(&mut self.shared.ring.lock().lanes);
        let tasks: Vec<VecDeque<Task>> =
            lanes.iter().map(|lane| std::mem::take(&mut lane.state.lock().tasks)).collect();
        drop(tasks);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_group_waits_for_its_tasks() {
        let pool = Pool::new(4);
        let group = TaskGroup::new(&pool);
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..50 {
            let count = Arc::clone(&count);
            group.spawn(move || {
                std::thread::sleep(Duration::from_millis(1));
                count.fetch_add(1, Ordering::SeqCst);
            });
        }
        group.wait();
        assert_eq!(count.load(Ordering::SeqCst), 50);
    }

    #[test]
    fn workers_start_as_tasks_arrive_up_to_the_limit() {
        let pool = Pool::new(3);
        let started = || pool.workers.shared.threads.lock().ids.len();
        assert_eq!(started(), 0);
        let group = TaskGroup::new(&pool);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (running, run) = mpsc::channel();
        for _ in 0..5 {
            let (gate, running) = (Arc::clone(&gate), running.clone());
            group.spawn(move || {
                running.send(()).unwrap();
                let (open, opened) = &*gate;
                let mut open = open.lock();
                while !*open {
                    opened.wait(&mut open);
                }
            });
        }
        for _ in 0..3 {
            run.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        assert_eq!(started(), 3);
        *gate.0.lock() = true;
        gate.1.notify_all();
        group.wait();
        assert_eq!(started(), 3);
    }

    #[test]
    fn a_group_waits_for_the_tasks_its_tasks_spawn() {
        let pool = Pool::new(2);
        let group = TaskGroup::new(&pool);
        let count = Arc::new(AtomicUsize::new(0));
        let (inner, counter) = (group.clone(), Arc::clone(&count));
        group.spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            inner.spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                counter.fetch_add(1, Ordering::SeqCst);
            });
        });
        group.wait();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_wait_runs_queued_tasks_on_the_waiting_thread() {
        let pool = Pool::new(1);
        let group = TaskGroup::new(&pool);
        let (blocked, block) = mpsc::channel::<()>();
        let (started, start) = mpsc::channel::<()>();
        // Holds the one worker.
        pool.spawn(move || {
            started.send(()).unwrap();
            let _ = block.recv();
        });
        start.recv().unwrap();
        let ran_on = Arc::new(Mutex::new(None));
        let on = Arc::clone(&ran_on);
        group.spawn(move || *on.lock() = Some(std::thread::current().id()));
        group.wait();
        assert_eq!(*ran_on.lock(), Some(std::thread::current().id()));
        drop(blocked);
    }

    #[test]
    fn workers_visit_the_lanes_in_turn() {
        let pool = Pool::local();
        let (busy, other) = (TaskGroup::new(&pool), TaskGroup::new(&pool));
        let order = Arc::new(Mutex::new(Vec::new()));
        for value in 0..3 {
            let order = Arc::clone(&order);
            busy.spawn(move || order.lock().push(value));
        }
        let o = Arc::clone(&order);
        other.spawn(move || o.lock().push(10));
        pool.run_all();
        assert_eq!(*order.lock(), vec![0, 10, 1, 2]);
    }

    #[test]
    fn a_local_pool_runs_when_its_host_runs_it() {
        let pool = Pool::local();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        pool.set_waker(Some(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        })));
        let count = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let count = Arc::clone(&count);
            pool.spawn(move || {
                count.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        // A past deadline stops after the first task.
        assert!(pool.run_until(Some(Instant::now())));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!pool.run_until(None));
        assert_eq!(count.load(Ordering::SeqCst), 3);
        pool.spawn(|| {});
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_wait_on_a_local_pool_runs_the_group_and_returns() {
        let pool = Pool::local();
        let group = TaskGroup::new(&pool);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (inner, o) = (group.clone(), Arc::clone(&order));
        group.spawn(move || {
            o.lock().push(1);
            // From a task of its own: it can't wait for itself.
            inner.wait();
            o.lock().push(3);
        });
        let o = Arc::clone(&order);
        group.spawn(move || o.lock().push(2));
        let o = Arc::clone(&order);
        pool.spawn(move || o.lock().push(4));
        group.wait();
        assert_eq!(*order.lock(), vec![1, 2, 3]);
        pool.run_all();
        assert_eq!(*order.lock(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_task_that_panics_still_ends() {
        let pool = Pool::new(1);
        let group = TaskGroup::new(&pool);
        group.spawn(|| panic!("a task panicked"));
        let (done, finished) = mpsc::channel();
        group.spawn(move || done.send(()).unwrap());
        finished.recv_timeout(Duration::from_secs(10)).unwrap();
        group.wait();
    }

    #[test]
    fn a_wait_that_runs_a_task_that_panics_returns() {
        let pool = Pool::local();
        let group = TaskGroup::new(&pool);
        let ran = Arc::new(AtomicUsize::new(0));
        group.spawn(|| panic!("a task panicked"));
        let r = Arc::clone(&ran);
        group.spawn(move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        group.spawn(|| panic!("another task panicked"));
        group.wait();
        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert!(!pool.run_until(None));
    }

    /// The last owner of a pool may be one of its own tasks.
    #[test]
    fn a_pool_dropped_by_its_own_task_stops() {
        let pool = Arc::new(Mutex::new(Some(Pool::new(2))));
        let (done, dropped) = mpsc::channel();
        let owner = Arc::clone(&pool);
        let spawner = pool.lock().clone().unwrap();
        drop(pool);
        spawner.spawn(move || {
            drop(owner.lock().take());
            done.send(()).unwrap();
        });
        drop(spawner);
        dropped.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn a_wait_from_a_worker_panics() {
        let pool = Pool::new(1);
        let group = TaskGroup::new(&pool);
        let (result, outcome) = mpsc::channel();
        let inner = group.clone();
        pool.spawn(move || {
            let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner.wait()));
            result.send(waited.is_err()).unwrap();
        });
        assert!(outcome.recv_timeout(Duration::from_secs(10)).unwrap());
    }
}
