//! Where background work runs: a [`Pool`] of worker threads, and the
//! [`TaskGroup`]s of its tasks that an owner waits for.
//!
//! A pool's tasks queue in lanes: the pool's own, and one per group. Its
//! workers take one task from each lane with work in turn, so a lane with
//! many tasks queued doesn't hold back the tasks of the others. A serial
//! group's lane ([`TaskGroup::serial`]) runs one task at a time, in order.
//!
//! A pool without threads ([`Pool::local`]) is for a host with one thread,
//! the browser: its tasks run when that thread calls [`Pool::run_until`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

use mln_style::time::Instant;
use parking_lot::{Condvar, Mutex};

#[cfg(not(target_family = "wasm"))]
#[cfg(not(target_family = "wasm"))]
use crate::thread::Joined;

/// A task that runs on a pool.
pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// What a pool calls when work arrives while it had none.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// What a pool calls with the message of a task's panic.
pub type PanicHandler = Arc<dyn Fn(String) + Send + Sync>;

/// Where a job hands its result: called at most once, from any thread,
/// while the job runs or later. Dropping it uncalled drops the result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

/// Runs background work on worker threads, or, without threads, on the
/// thread that runs it. Cheap to clone: clones share the pool. A worker
/// starts when a task arrives while every running worker is busy, up to the
/// pool's thread count, so a pool that runs nothing starts no threads. Its
/// threads end once the last clone and the last [`TaskGroup`] of it drop, or
/// once it's [`shut_down`](Self::shut_down), dropping the tasks still
/// queued.
#[derive(Clone)]
pub struct Pool {
    workers: Arc<Workers>,
}

/// A lane of tasks of one pool, which an owner waits for with
/// [`wait`](Self::wait). Cheap to clone: clones share the lane. Dropping it
/// leaves its queued tasks to run.
///
/// A [`serial`](Self::serial) group runs at most one of its tasks at a time,
/// in the order they were spawned.
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
    panic_handler: Mutex<Option<PanicHandler>>,
    /// The most worker threads the pool starts: none for a local pool.
    max_threads: usize,
    /// The workers started so far.
    threads: Mutex<Threads>,
    /// Whether the pool stopped. Set under the ring's lock, which the workers
    /// wait on, and read without it by threads that hold a lane's lock.
    stopped: AtomicBool,
}

#[derive(Default)]
struct Threads {
    #[cfg(not(target_family = "wasm"))]
    joined: Vec<Joined>,
    ids: Vec<ThreadId>,
}

/// The lanes that may start a task, each once, in the order the workers
/// visit them. A lane may also be here for a while after it can no longer
/// start one (a waiting thread took its task); the workers drop it then.
#[derive(Default)]
struct Ring {
    lanes: VecDeque<Arc<Lane>>,
    /// The workers started, or starting.
    started: usize,
    /// The workers waiting for a task.
    idle: usize,
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
    /// Whether the lane runs one task at a time. Fixed when it's made.
    serial: bool,
    /// Whether the lane is in the ring, or the thread that set this is about
    /// to put it there.
    ///
    /// The lane's invariant: whenever its lock is released, a lane that
    /// [`may_start`](Self::may_start) a task has this set. Whatever makes a
    /// lane able to start a task (a push, or a serial lane's task ending)
    /// sets it and puts the lane in the ring if it wasn't; only a thread
    /// that finds the lane unable to start one clears it, as it takes the
    /// lane from the ring.
    in_ring: bool,
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

    /// Stops the pool now, whatever clones and groups of it are left, as when
    /// the last of them drops: its workers end once their current tasks
    /// return, the tasks waiting for a worker drop, and tasks spawned later
    /// drop at once. For an owner whose work may still be held where it
    /// can't drop, such as a renderer forgotten with its graphics.
    pub fn shut_down(&self) {
        self.workers.shared.stop();
    }

    /// Calls `waker`, from the spawning thread, whenever a task arrives
    /// while none were queued, so that the host of a local pool knows to
    /// run it.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.workers.shared.waker.lock() = waker;
    }

    /// Calls `handler` with the message of each task that panics, after the
    /// panic hook reported it. The task ends as any other, and the pool goes
    /// on, so the owner of what the task worked on fails it.
    pub fn set_panic_handler(&self, handler: Option<PanicHandler>) {
        *self.workers.shared.panic_handler.lock() = handler;
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
            shared.run(&lane, task);
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

    /// A new lane of `pool` that runs at most one of its tasks at a time, in
    /// the order they were spawned, on whichever thread takes each. A task
    /// that one of its tasks spawns into the group runs after that task.
    #[must_use]
    pub fn serial(pool: &Pool) -> TaskGroup {
        let lane = Lane { state: Mutex::new(LaneState { serial: true, ..LaneState::default() }), ..Lane::default() };
        TaskGroup { pool: pool.clone(), lane: Arc::new(lane) }
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
    /// workers. A [`serial`](Self::serial) group's next task runs here only
    /// once none of its tasks runs elsewhere. On a [`local`](Pool::local)
    /// pool it doesn't wait: a task that runs there runs on this thread,
    /// below this call, so a serial group's tasks queued behind it stay
    /// queued.
    ///
    /// # Panics
    ///
    /// On one of the pool's workers, which a running task could be waiting
    /// for.
    pub fn wait(&self) {
        assert!(!self.pool.is_worker(), "a task group was waited on from a task of its pool");
        let shared = &self.pool.workers.shared;
        let waits = self.pool.threads() > 0;
        let lane = &self.lane;
        let mut state = lane.state.lock();
        loop {
            if shared.is_stopped() {
                // A stopped pool starts no task: those queued drop.
                let dropped = std::mem::take(&mut state.tasks);
                drop(state);
                drop(dropped);
                state = lane.state.lock();
                if state.running == 0 || !waits {
                    return;
                }
                lane.ended.wait(&mut state);
            } else if let Some(task) = state.start() {
                drop(state);
                shared.run(lane, task);
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
            panic_handler: Mutex::new(None),
            max_threads,
            threads: Mutex::default(),
            stopped: AtomicBool::new(false),
        }
    }

    fn push(self: &Arc<Self>, lane: &Arc<Lane>, task: Task) {
        if self.is_stopped() {
            return;
        }
        let (joins, startable) = {
            let mut state = lane.state.lock();
            state.tasks.push_back(task);
            let joins = state.join();
            // A serial lane's task that doesn't join the ring waits for the
            // lane's other tasks, not for a worker.
            (joins, joins || !state.serial)
        };
        let (was_idle, start) = {
            let mut ring = self.ring.lock();
            let was_idle = joins && self.enter(&mut ring, lane);
            // Every worker is busy: another starts, if the pool may run one.
            let start = startable && ring.idle == 0 && !self.is_stopped() && ring.started < self.max_threads;
            ring.started += usize::from(start);
            (was_idle, start)
        };
        if start {
            self.start_worker();
        }
        if was_idle {
            self.wake();
        }
    }

    /// Puts `lane`, which [`LaneState::join`] marked, at the back of the
    /// ring, and returns whether the ring was empty.
    fn enter(&self, ring: &mut Ring, lane: &Arc<Lane>) -> bool {
        let was_empty = ring.lanes.is_empty();
        ring.lanes.push_back(Arc::clone(lane));
        self.work.notify_one();
        was_empty
    }

    /// Calls the waker, if the pool has one.
    fn wake(&self) {
        let waker = self.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Takes the next task, from the first lane in the ring that may start
    /// one, which goes to the back if it may start another. The lanes before
    /// it, which may not, leave the ring. Its lane counts it running.
    fn next(&self, ring: &mut Ring) -> Option<(Arc<Lane>, Task)> {
        if self.is_stopped() {
            return None;
        }
        while let Some(lane) = ring.lanes.pop_front() {
            let mut state = lane.state.lock();
            let Some(task) = state.start() else {
                state.in_ring = false;
                continue;
            };
            if state.may_start() {
                ring.lanes.push_back(Arc::clone(&lane));
                // Another worker takes the lane's next task.
                self.work.notify_one();
            } else {
                state.in_ring = false;
            }
            drop(state);
            return Some((lane, task));
        }
        None
    }

    /// Runs `task`, which `lane` counts running, and counts it ended once it
    /// and its captures are gone, even if it panics: the panic goes to the
    /// pool's panic handler instead of up this thread. A serial lane with tasks
    /// queued goes back in the ring then. No worker starts for it: the
    /// thread that ran the task goes on to take tasks, a worker's or
    /// [`Pool::run_until`]'s from the ring, a [`TaskGroup::wait`]'s from
    /// the lane.
    fn run(&self, lane: &Arc<Lane>, task: Task) {
        struct Ended<'a>(&'a Shared, &'a Arc<Lane>);
        impl Drop for Ended<'_> {
            fn drop(&mut self) {
                let Ended(shared, lane) = *self;
                let (joins, dropped) = {
                    let mut state = lane.state.lock();
                    state.running -= 1;
                    // On a stopped pool, the tasks a serial lane held back
                    // behind this one drop rather than start.
                    let dropped = if shared.is_stopped() { std::mem::take(&mut state.tasks) } else { VecDeque::new() };
                    (state.join(), dropped)
                };
                lane.ended.notify_all();
                drop(dropped);
                if joins {
                    // The ring's lock is released before the waker, which may
                    // spawn.
                    let was_empty = shared.enter(&mut shared.ring.lock(), lane);
                    if was_empty {
                        shared.wake();
                    }
                }
            }
        }
        let _ended = Ended(self, lane);
        if let Err(message) = crate::panic::catch(task) {
            let handler = self.panic_handler.lock().clone();
            if let Some(handler) = handler {
                handler(message);
            }
        }
    }

    /// Whether the pool stopped.
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Stops the workers once their tasks return, and waits for them unless
    /// this runs on a library thread (`thread::Joined`), such as one of them.
    /// The tasks waiting in the ring drop, and those spawned later drop at
    /// once.
    fn stop(&self) {
        {
            let _ring = self.ring.lock();
            self.stopped.store(true, Ordering::SeqCst);
        }
        self.work.notify_all();
        // Joined outside the lock, which a task that spawns takes.
        let threads = std::mem::take(&mut *self.threads.lock());
        drop(threads);
        // The tasks left, dropped here rather than with the last worker.
        let lanes = std::mem::take(&mut self.ring.lock().lanes);
        let tasks: Vec<VecDeque<Task>> =
            lanes.iter().map(|lane| std::mem::take(&mut lane.state.lock().tasks)).collect();
        drop(tasks);
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
                    log::error!(target: crate::log_target::GENERAL, "can't start a pool worker: {error}");
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
                    if self.is_stopped() {
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
            self.run(&lane, task);
        }
    }
}

impl LaneState {
    /// Whether the lane has a task queued that it may start now: any, or for
    /// a serial lane, one while none of its tasks runs.
    fn may_start(&self) -> bool {
        !self.tasks.is_empty() && (!self.serial || self.running == 0)
    }

    /// Takes the next task, which the lane counts running, if it may start
    /// one.
    fn start(&mut self) -> Option<Task> {
        if !self.may_start() {
            return None;
        }
        self.running += 1;
        self.tasks.pop_front()
    }

    /// Marks the lane in the ring if it may start a task and isn't marked.
    /// Returns whether it did: the caller then puts it there with
    /// [`Shared::enter`].
    fn join(&mut self) -> bool {
        let joins = !self.in_ring && self.may_start();
        self.in_ring |= joins;
        joins
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.shared.stop();
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
    fn a_task_that_panics_tells_the_panic_handler() {
        let pool = Pool::new(1);
        let (report, reported) = mpsc::channel();
        pool.set_panic_handler(Some(Arc::new(move |message| report.send(message).unwrap())));
        pool.spawn(|| panic!("a task panicked"));
        assert_eq!(reported.recv_timeout(Duration::from_secs(10)).unwrap(), "a task panicked");
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
    fn a_serial_group_runs_its_tasks_one_at_a_time_in_order() {
        let pool = Pool::new(4);
        let group = TaskGroup::serial(&pool);
        let (active, overlapped) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let order = Arc::new(Mutex::new(Vec::new()));
        let (gate, open) = mpsc::channel::<()>();
        let (started, start) = mpsc::channel();
        group.spawn(move || {
            started.send(()).unwrap();
            let _ = open.recv();
        });
        start.recv_timeout(Duration::from_secs(10)).unwrap();
        for value in 0..40 {
            let (active, overlapped, order) = (Arc::clone(&active), Arc::clone(&overlapped), Arc::clone(&order));
            group.spawn(move || {
                if active.fetch_add(1, Ordering::SeqCst) > 0 {
                    overlapped.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_micros(200));
                order.lock().push(value);
                active.fetch_sub(1, Ordering::SeqCst);
                // It ends as any other: the next task still runs.
                assert!(value != 20, "a task panicked");
            });
        }
        // While a task runs, the lane, which can't start another, is out of
        // the ring, and the queued tasks start no workers.
        assert!(pool.workers.shared.ring.lock().lanes.is_empty());
        assert_eq!(pool.workers.shared.threads.lock().ids.len(), 1);
        drop(gate);
        let (done, finished) = mpsc::channel();
        group.spawn(move || done.send(()).unwrap());
        finished.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(*order.lock(), (0..40).collect::<Vec<_>>());
        assert_eq!(overlapped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_serial_task_spawns_into_its_group_behind_itself() {
        let pool = Pool::new(4);
        let group = TaskGroup::serial(&pool);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (inner, o) = (group.clone(), Arc::clone(&order));
        group.spawn(move || {
            let o2 = Arc::clone(&o);
            inner.spawn(move || o2.lock().push(2));
            std::thread::sleep(Duration::from_millis(20));
            o.lock().push(1);
        });
        group.wait();
        assert_eq!(*order.lock(), vec![1, 2]);
    }

    #[test]
    fn a_wait_on_a_serial_group_runs_nothing_while_one_of_its_tasks_runs() {
        let pool = Pool::new(1);
        let group = TaskGroup::serial(&pool);
        let ended = Arc::new(AtomicUsize::new(0));
        let (started, start) = mpsc::channel();
        let e = Arc::clone(&ended);
        group.spawn(move || {
            started.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            e.fetch_add(1, Ordering::SeqCst);
        });
        start.recv_timeout(Duration::from_secs(10)).unwrap();
        let saw = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..3 {
            let (e, saw) = (Arc::clone(&ended), Arc::clone(&saw));
            group.spawn(move || {
                let before = e.load(Ordering::SeqCst);
                saw.lock().push(before);
                e.fetch_add(1, Ordering::SeqCst);
            });
        }
        group.wait();
        assert_eq!(*saw.lock(), vec![1, 2, 3]);
    }

    #[test]
    fn a_serial_group_on_a_local_pool_runs_in_order() {
        let pool = Pool::local();
        let group = TaskGroup::serial(&pool);
        let order = Arc::new(Mutex::new(Vec::new()));
        let (inner, o) = (group.clone(), Arc::clone(&order));
        group.spawn(move || {
            o.lock().push(1);
            // Its group's next task can't start while this one runs.
            inner.wait();
            let o2 = Arc::clone(&o);
            inner.spawn(move || o2.lock().push(5));
            o.lock().push(2);
        });
        group.spawn(|| panic!("a task panicked"));
        for value in [3, 4] {
            let o = Arc::clone(&order);
            group.spawn(move || o.lock().push(value));
        }
        assert!(pool.run_until(Some(Instant::now())));
        assert_eq!(*order.lock(), vec![1, 2]);
        group.wait();
        assert_eq!(*order.lock(), vec![1, 2, 3, 4, 5]);
        assert!(!pool.run_until(None));
    }

    #[test]
    fn a_pool_shut_down_while_a_clone_lives_stops_its_workers() {
        let pool = Pool::new(2);
        let (ran, first) = mpsc::channel();
        pool.spawn(move || ran.send(()).unwrap());
        first.recv_timeout(Duration::from_secs(10)).unwrap();
        let worker = pool.workers.shared.threads.lock().joined.len();
        assert_eq!(worker, 1);
        // As a renderer forgotten with its graphics holds its pool.
        std::mem::forget(pool.clone());
        pool.shut_down();
        assert!(pool.workers.shared.threads.lock().joined.is_empty());
        let (later, dropped) = mpsc::channel::<()>();
        pool.spawn(move || later.send(()).unwrap());
        assert!(dropped.recv_timeout(Duration::from_secs(1)).is_err(), "a task ran after the shutdown");
    }

    #[test]
    fn a_shut_down_drops_the_tasks_behind_a_running_serial_task() {
        let pool = Pool::new(1);
        let group = TaskGroup::serial(&pool);
        let (started, start) = mpsc::channel();
        let (release, blocked) = mpsc::channel::<()>();
        group.spawn(move || {
            started.send(()).unwrap();
            let _ = blocked.recv();
        });
        start.recv_timeout(Duration::from_secs(10)).unwrap();
        let ran = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&ran);
        group.spawn(move || {
            r.fetch_add(1, Ordering::SeqCst);
        });
        // From a library thread, as a runtime's: it doesn't wait for the
        // running task.
        let shut = pool.clone();
        crate::thread::spawn("shut down", move || shut.shut_down()).unwrap().join().unwrap();
        drop(release);
        group.wait();
        assert_eq!(ran.load(Ordering::SeqCst), 0);
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
