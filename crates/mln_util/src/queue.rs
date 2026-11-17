//! The work of a thread that owns some state: a [`Queue`] of tasks that take
//! the state by `&mut`, posted from any thread and run by the owning thread
//! in its turns.
//!
//! A task can be posted to run as soon as the owner runs the queue, or once
//! a deadline passes. A thread of its own runs the queue with
//! [`Queue::run`], which parks until a task arrives or the next deadline
//! passes. A host that has its own loop, such as the browser's pump, runs it
//! with [`Queue::run_once`] instead, and learns of new work through the
//! queue's waker.

use std::cmp::Ordering as CmpOrdering;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

use crate::time::Instant;

/// A task that runs with the queue's state.
type Task<S> = Box<dyn FnOnce(&mut S) + Send>;

/// What a queue calls, on the posting thread, when work arrives.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A queue of tasks that take `&mut S`, which the thread that runs the queue
/// owns. Cheap to clone: clones are handles to the same queue.
///
/// Tasks run in the order they were posted, and a timed task once its
/// deadline passed, before the tasks of the same turn. A task that a closed
/// queue gets drops at once, so tasks that hold handles to their own queue
/// can't keep it alive once its owner closed it.
pub struct Queue<S> {
    inner: Arc<Inner<S>>,
}

/// A handle to a [`Queue`] that doesn't keep it alive: what it posts once
/// the queue is gone drops.
pub struct WeakQueue<S> {
    inner: Weak<Inner<S>>,
}

struct Inner<S> {
    state: Mutex<State<S>>,
    /// Signalled when a task arrives or the queue is woken.
    signal: Condvar,
    waker: Mutex<Option<Waker>>,
}

struct State<S> {
    tasks: VecDeque<Entry<S>>,
    timed: BinaryHeap<Timed<S>>,
    /// Counts the timed tasks, so that those with the same deadline run in
    /// the order they were posted.
    posted: u64,
    /// A task arrived or the queue was woken since the owner last parked.
    signalled: bool,
    /// A stop task ran since the last [`Queue::run`] began.
    stopped: bool,
    closed: bool,
}

enum Entry<S> {
    Task(Task<S>),
    Stop,
}

struct Timed<S> {
    deadline: Instant,
    order: u64,
    task: Task<S>,
}

impl<S> PartialEq for Timed<S> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}

impl<S> Eq for Timed<S> {}

impl<S> PartialOrd for Timed<S> {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl<S> Ord for Timed<S> {
    /// Reversed, so that the heap's greatest entry is the earliest.
    fn cmp(&self, other: &Self) -> CmpOrdering {
        (other.deadline, other.order).cmp(&(self.deadline, self.order))
    }
}

impl<S> Clone for Queue<S> {
    fn clone(&self) -> Self {
        Queue { inner: Arc::clone(&self.inner) }
    }
}

impl<S> Clone for WeakQueue<S> {
    fn clone(&self) -> Self {
        WeakQueue { inner: Weak::clone(&self.inner) }
    }
}

impl<S> Default for Queue<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Queue<S> {
    #[must_use]
    pub fn new() -> Self {
        Queue {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    tasks: VecDeque::new(),
                    timed: BinaryHeap::new(),
                    posted: 0,
                    signalled: false,
                    stopped: false,
                    closed: false,
                }),
                signal: Condvar::new(),
                waker: Mutex::new(None),
            }),
        }
    }

    /// Runs `task` on the queue's thread, after the tasks posted before it.
    pub fn post(&self, task: impl FnOnce(&mut S) + Send + 'static) {
        self.inner.push(Entry::Task(Box::new(task)));
    }

    /// Runs `task` on the queue's thread in its first turn once `deadline`
    /// passed, before the turn's other tasks.
    pub fn post_at(&self, deadline: Instant, task: impl FnOnce(&mut S) + Send + 'static) {
        let mut state = self.inner.state.lock();
        if state.closed {
            drop(state);
            drop(task);
            return;
        }
        let order = state.posted;
        state.posted += 1;
        state.timed.push(Timed { deadline, order, task: Box::new(task) });
        drop(state);
        self.wake();
    }

    /// [`post_at`](Self::post_at) `delay` from now. A delay past what an
    /// `Instant` can hold never comes.
    pub fn post_after(&self, delay: Duration, task: impl FnOnce(&mut S) + Send + 'static) {
        if let Some(deadline) = Instant::now().checked_add(delay) {
            self.post_at(deadline, task);
        }
    }

    /// Calls `waker`, from the posting thread, whenever a task arrives or
    /// the queue is woken, so that a host that runs the queue with
    /// [`run_once`](Self::run_once) knows to run it. A timed task calls it
    /// when it's posted, not when its deadline passes.
    pub fn set_waker(&self, waker: Option<Waker>) {
        *self.inner.waker.lock() = waker;
    }

    /// Wakes the queue's thread, as a task would.
    pub fn wake(&self) {
        self.inner.wake();
    }

    /// Runs the timed tasks whose deadline passed, then the tasks waiting
    /// now and those they post, with `state`, until none waits. Returns
    /// whether any task ran.
    pub fn run_once(&self, state: &mut S) -> bool {
        let mut ran = false;
        let now = Instant::now();
        loop {
            let task = {
                let mut queue = self.inner.state.lock();
                if queue.closed {
                    return ran;
                }
                match queue.timed.peek() {
                    Some(timed) if timed.deadline <= now => queue.timed.pop().map(|timed| timed.task),
                    _ => None,
                }
            };
            let Some(task) = task else { break };
            task(state);
            ran = true;
        }
        loop {
            let entry = {
                let mut queue = self.inner.state.lock();
                if queue.closed {
                    return ran;
                }
                queue.tasks.pop_front()
            };
            match entry {
                Some(Entry::Task(task)) => task(state),
                Some(Entry::Stop) => self.inner.state.lock().stopped = true,
                None => return ran,
            }
            ran = true;
        }
    }

    /// Whether a task waits for [`run_once`](Self::run_once), or a timed
    /// task's deadline passed.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        let state = self.inner.state.lock();
        !state.tasks.is_empty() || state.timed.peek().is_some_and(|timed| timed.deadline <= Instant::now())
    }

    /// The deadline of the earliest timed task.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.inner.state.lock().timed.peek().map(|timed| timed.deadline)
    }

    /// Sleeps until a task arrives or the queue is woken, or the next timed
    /// task is due. It returns at once when that happened since it last
    /// returned, as while the queue was running.
    pub fn park(&self) {
        self.park_until(None);
    }

    /// [`park`](Self::park), but returns at `deadline` at the latest.
    pub fn park_until(&self, deadline: Option<Instant>) {
        let mut state = self.inner.state.lock();
        let deadline = match (deadline, state.timed.peek().map(|timed| timed.deadline)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        while !state.signalled {
            match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return;
                    }
                    self.inner.signal.wait_for(&mut state, deadline - now);
                }
                None => self.inner.signal.wait(&mut state),
            }
        }
        state.signalled = false;
    }

    /// Runs the queue with `state` on this thread until a stop task runs,
    /// parking while it has nothing to do: what a thread of its own does.
    pub fn run(&self, state: &mut S) {
        self.run_with(state, |_| {});
    }

    /// [`run`](Self::run), and `each_turn` after each turn, before the
    /// queue parks, for an owner with work of its own on this thread, which
    /// wakes the queue when it has some.
    pub fn run_with(&self, state: &mut S, mut each_turn: impl FnMut(&mut S)) {
        self.inner.state.lock().stopped = false;
        loop {
            self.run_once(state);
            each_turn(state);
            if self.is_stopped() {
                return;
            }
            self.park();
        }
    }

    /// Ends the [`run`](Self::run) in progress, or the next one, at the end
    /// of the turn that runs this stop task: once the tasks posted before it
    /// ran, and those that wait in the same turn after it.
    pub fn stop(&self) {
        self.inner.push(Entry::Stop);
    }

    /// Whether a stop task ran since the last [`run`](Self::run) began, for
    /// a host that runs the queue with [`run_once`](Self::run_once).
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.inner.state.lock().stopped
    }

    /// Drops the tasks that wait, and from now on every task posted, for an
    /// owner that runs the queue no more.
    pub fn close(&self) {
        let (tasks, timed) = {
            let mut state = self.inner.state.lock();
            state.closed = true;
            (std::mem::take(&mut state.tasks), std::mem::take(&mut state.timed))
        };
        // Outside the lock: a task's drop may post to the queue.
        drop(tasks);
        drop(timed);
    }

    /// A handle that doesn't keep the queue alive.
    #[must_use]
    pub fn downgrade(&self) -> WeakQueue<S> {
        WeakQueue { inner: Arc::downgrade(&self.inner) }
    }
}

impl<S> WeakQueue<S> {
    /// The queue, unless it's gone.
    #[must_use]
    pub fn upgrade(&self) -> Option<Queue<S>> {
        self.inner.upgrade().map(|inner| Queue { inner })
    }
}

impl<S> Inner<S> {
    fn push(&self, entry: Entry<S>) {
        let mut state = self.state.lock();
        if state.closed {
            drop(state);
            drop(entry);
            return;
        }
        state.tasks.push_back(entry);
        drop(state);
        self.wake();
    }

    fn wake(&self) {
        self.state.lock().signalled = true;
        self.signal.notify_all();
        let waker = self.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn tasks_run_in_order_with_the_state() {
        let queue = Queue::<Vec<u32>>::new();
        for i in 0..5 {
            queue.post(move |state| state.push(i));
        }
        let mut state = Vec::new();
        assert!(queue.run_once(&mut state));
        assert_eq!(state, [0, 1, 2, 3, 4]);
        assert!(!queue.run_once(&mut state));
    }

    #[test]
    fn a_task_posted_by_a_task_runs_in_the_same_turn() {
        let queue = Queue::<u32>::new();
        let again = queue.clone();
        queue.post(move |count| {
            *count += 1;
            again.post(|count| *count += 1);
        });
        let mut count = 0;
        queue.run_once(&mut count);
        assert_eq!(count, 2);
    }

    #[test]
    fn timed_tasks_run_once_due_earliest_first_before_the_turns_tasks() {
        let queue = Queue::<Vec<&str>>::new();
        let now = Instant::now();
        queue.post_at(now + Duration::from_millis(20), |state| state.push("later"));
        queue.post_at(now, |state| state.push("due"));
        queue.post_at(now, |state| state.push("due too"));
        queue.post(|state| state.push("task"));
        let mut state = Vec::new();
        queue.run_once(&mut state);
        assert_eq!(state, ["due", "due too", "task"]);
        assert_eq!(queue.next_deadline(), Some(now + Duration::from_millis(20)));
        queue.park();
        // The posts signalled the queue, so the first park returns at once.
        queue.park();
        assert!(Instant::now() >= now + Duration::from_millis(20));
        queue.run_once(&mut state);
        assert_eq!(state, ["due", "due too", "task", "later"]);
    }

    #[test]
    fn run_returns_after_the_turn_of_its_stop_and_runs_again() {
        let queue = Queue::<u32>::new();
        queue.post(|count| *count += 1);
        queue.stop();
        queue.post(|count| *count += 10);
        let mut count = 0;
        queue.run(&mut count);
        assert_eq!(count, 11);
        assert!(queue.is_stopped());
        queue.stop();
        queue.post(|count| *count += 100);
        queue.run(&mut count);
        assert_eq!(count, 111);
    }

    #[test]
    fn run_from_another_thread_waits_for_its_tasks() {
        let queue = Queue::<u32>::new();
        let handle = queue.clone();
        let thread = std::thread::spawn(move || {
            let mut count = 0;
            handle.run(&mut count);
            count
        });
        for _ in 0..10 {
            queue.post(|count| *count += 1);
        }
        queue.post_after(Duration::from_millis(5), |count| *count += 100);
        queue.post_after(Duration::from_millis(10), |_| {});
        let stop = queue.clone();
        queue.post_after(Duration::from_millis(10), move |_| stop.stop());
        assert_eq!(thread.join().unwrap(), 110);
    }

    #[test]
    fn a_closed_queue_drops_what_waits_and_what_comes() {
        struct Counted(Arc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let queue = Queue::<()>::new();
        let (waiting, timed) = (Counted(Arc::clone(&dropped)), Counted(Arc::clone(&dropped)));
        // Each task holds a handle to its own queue.
        let own = queue.clone();
        queue.post(move |()| drop((waiting, own)));
        queue.post_after(Duration::from_secs(60), move |()| drop(timed));
        queue.close();
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
        let late = Counted(Arc::clone(&dropped));
        queue.post(move |()| drop(late));
        assert_eq!(dropped.load(Ordering::SeqCst), 3);
        assert!(!queue.run_once(&mut ()));
        let weak = queue.downgrade();
        drop(queue);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn the_waker_hears_of_posts_and_wakes() {
        let woken = Arc::new(AtomicUsize::new(0));
        let queue = Queue::<()>::new();
        let counter = Arc::clone(&woken);
        queue.set_waker(Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        queue.post(|()| {});
        queue.wake();
        queue.post_after(Duration::from_secs(60), |()| {});
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        assert!(queue.has_pending());
        queue.run_once(&mut ());
        assert!(!queue.has_pending());
    }
}
