//! The work of a thread that owns some state: a [`Queue`] of tasks that take
//! the state by `&mut`, posted from any thread and run by the owning thread
//! in its turns.
//!
//! A task can be posted to run as soon as the owner runs the queue, or once
//! a deadline passes, and a timed task can be cancelled until it runs. A
//! thread of its own runs the queue with [`Queue::run`], which parks until a
//! task arrives or the next deadline passes. A host that has its own loop,
//! such as the browser's pump, runs it with [`Queue::run_once`] instead, and
//! learns of new work through the queue's waker.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::Duration;

use mln_style::hash::HashMap;
use mln_style::time::Instant;
use parking_lot::{Condvar, Mutex};

/// A task that runs with the queue's state.
type Task<S> = Box<dyn FnOnce(&mut S) + Send>;

/// What a queue calls, on the posting thread, when work arrives.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How many cancelled deadlines the heap keeps, beyond one per waiting timed
/// task, before it drops them all.
const CANCELLED_SLACK: usize = 64;

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

/// Names a timed task of a [`Queue`], for [`Queue::cancel`]. Each one that
/// a queue hands out names a task of its own, if any: a timed task that a
/// closed queue got, or whose deadline never comes, has a key that names
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimedTask(u64);

struct Inner<S> {
    state: Mutex<State<S>>,
    /// Signalled when a task arrives or the queue is woken, or a timed task
    /// is cancelled.
    signal: Condvar,
    waker: Mutex<Option<Waker>>,
}

struct State<S> {
    tasks: VecDeque<Entry<S>>,
    /// The timed tasks that wait, by key.
    timed: HashMap<u64, Task<S>>,
    /// The deadlines of the timed tasks, earliest first, and of those due
    /// at once, the first posted first. A cancelled task's deadline stays
    /// until it reaches the top, or until the deadlines number more than
    /// twice the waiting tasks plus [`CANCELLED_SLACK`].
    deadlines: BinaryHeap<Reverse<(Instant, u64)>>,
    /// Counts the timed tasks, for their keys.
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

impl<S> State<S> {
    /// The deadline and key of the earliest timed task that waits, after
    /// dropping the cancelled deadlines before it.
    fn next_timed(&mut self) -> Option<(Instant, u64)> {
        while let Some(&Reverse((deadline, key))) = self.deadlines.peek() {
            if self.timed.contains_key(&key) {
                return Some((deadline, key));
            }
            self.deadlines.pop();
        }
        None
    }

    /// Takes the earliest timed task whose deadline is `now` or before.
    fn take_due(&mut self, now: Instant) -> Option<Task<S>> {
        let (deadline, key) = self.next_timed()?;
        if deadline > now {
            return None;
        }
        self.deadlines.pop();
        self.timed.remove(&key)
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
                    timed: HashMap::default(),
                    deadlines: BinaryHeap::new(),
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
    /// passed, before the turn's other tasks, unless it's
    /// [`cancel`](Self::cancel)led first.
    pub fn post_at(&self, deadline: Instant, task: impl FnOnce(&mut S) + Send + 'static) -> TimedTask {
        let mut state = self.inner.state.lock();
        let key = state.posted;
        state.posted += 1;
        if state.closed {
            drop(state);
            drop(task);
            return TimedTask(key);
        }
        state.timed.insert(key, Box::new(task));
        state.deadlines.push(Reverse((deadline, key)));
        drop(state);
        self.wake();
        TimedTask(key)
    }

    /// [`post_at`](Self::post_at) `delay` from now. A delay past what an
    /// `Instant` can hold never comes.
    pub fn post_after(&self, delay: Duration, task: impl FnOnce(&mut S) + Send + 'static) -> TimedTask {
        if let Some(deadline) = Instant::now().checked_add(delay) {
            return self.post_at(deadline, task);
        }
        let mut state = self.inner.state.lock();
        state.posted += 1;
        TimedTask(state.posted - 1)
    }

    /// Drops the timed task that `task` names, on this thread, unless it
    /// has begun to run or dropped already: it never runs, and from now on
    /// [`next_deadline`](Self::next_deadline) and the queue's parking no
    /// longer wait for it. A host that armed a timeout for its deadline
    /// before still sees that timeout end.
    pub fn cancel(&self, task: TimedTask) {
        let cancelled = {
            let mut state = self.inner.state.lock();
            let cancelled = state.timed.remove(&task.0);
            if state.deadlines.len() > 2 * state.timed.len() + CANCELLED_SLACK {
                let State { timed, deadlines, .. } = &mut *state;
                deadlines.retain(|Reverse((_, key))| timed.contains_key(key));
            }
            cancelled
        };
        if cancelled.is_some() {
            // A thread that parks until the task's deadline waits for the
            // next one instead.
            self.inner.signal.notify_all();
        }
        // Outside the lock: a task's drop may post to the queue.
        drop(cancelled);
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
        self.run_once_by(state, Instant::now())
    }

    /// [`run_once`](Self::run_once), with the timed tasks whose deadline is
    /// `now` or before taken as passed, whatever the time is: for a caller
    /// that keeps time by a clock of its own, such as the benchmarks'
    /// simulated network, whose deadlines it gives by that clock too.
    #[cfg(any(test, feature = "bench"))]
    #[doc(hidden)]
    pub fn run_once_at(&self, state: &mut S, now: Instant) -> bool {
        self.run_once_by(state, now)
    }

    fn run_once_by(&self, state: &mut S, now: Instant) -> bool {
        let mut ran = false;
        loop {
            let task = {
                let mut queue = self.inner.state.lock();
                if queue.closed {
                    return ran;
                }
                queue.take_due(now)
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
        let mut state = self.inner.state.lock();
        !state.tasks.is_empty() || state.next_timed().is_some_and(|(deadline, _)| deadline <= Instant::now())
    }

    /// The deadline of the earliest timed task that waits, which a
    /// cancelled one no longer does.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.inner.state.lock().next_timed().map(|(deadline, _)| deadline)
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
        while !state.signalled {
            // Again after each wait: a cancel may have taken the earliest.
            let deadline = match (deadline, state.next_timed().map(|(deadline, _)| deadline)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
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
            state.deadlines.clear();
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
        let later = now + Duration::from_secs(3600);
        queue.post_at(later, |state| state.push("later"));
        queue.post_at(now, |state| state.push("due"));
        queue.post_at(now, |state| state.push("due too"));
        queue.post(|state| state.push("task"));
        let mut state = Vec::new();
        queue.run_once(&mut state);
        assert_eq!(state, ["due", "due too", "task"]);
        assert_eq!(queue.next_deadline(), Some(later));
        let soon = Instant::now() + Duration::from_millis(20);
        queue.post_at(soon, |state| state.push("soon"));
        queue.park();
        // The posts signalled the queue, so the first park returns at once.
        queue.park();
        assert!(Instant::now() >= soon);
        queue.run_once(&mut state);
        assert_eq!(state, ["due", "due too", "task", "soon"]);
        assert_eq!(queue.next_deadline(), Some(later));
    }

    #[test]
    fn a_cancelled_timed_task_drops_and_never_runs() {
        struct Counted(Arc<AtomicUsize>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let queue = Queue::<Vec<&str>>::new();
        let now = Instant::now();
        let counted = Counted(Arc::clone(&dropped));
        let cancelled = queue.post_at(now, move |state| {
            drop(counted);
            state.push("cancelled");
        });
        let kept = queue.post_at(now, |state| state.push("kept"));
        queue.cancel(cancelled);
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "the cancel drops the task");
        let mut state = Vec::new();
        queue.run_once(&mut state);
        assert_eq!(state, ["kept"]);
        // A key whose task ran, or that was cancelled, names nothing.
        queue.cancel(kept);
        queue.cancel(cancelled);
        assert!(!queue.run_once(&mut state));
    }

    #[test]
    fn next_deadline_and_park_skip_cancelled_tasks() {
        let queue = Queue::<()>::new();
        let now = Instant::now();
        let soon = queue.post_at(now + Duration::from_millis(5), |()| {});
        let later = now + Duration::from_secs(60);
        queue.post_at(later, |()| {});
        assert_eq!(queue.next_deadline(), Some(now + Duration::from_millis(5)));
        queue.cancel(soon);
        assert_eq!(queue.next_deadline(), Some(later));
        assert!(!queue.has_pending());
        // The posts signalled the queue; the second park waits for the
        // cancelled task's deadline no more, only for the bound.
        queue.park();
        let bound = Instant::now() + Duration::from_millis(30);
        queue.park_until(Some(bound));
        assert!(Instant::now() >= bound);
        assert!(!queue.run_once(&mut ()));
    }

    /// A parked thread whose earliest timed task is cancelled waits for the
    /// next one instead.
    #[test]
    fn a_cancel_moves_a_parked_threads_deadline() {
        let queue = Queue::<()>::new();
        let now = Instant::now();
        let due = now + Duration::from_millis(100);
        let bound = now + Duration::from_millis(150);
        let first = queue.post_at(due, |()| {});
        let parked = queue.clone();
        let thread = std::thread::spawn(move || {
            // The post signalled the queue, so the first park returns at once.
            parked.park();
            parked.park_until(Some(bound));
            Instant::now()
        });
        std::thread::sleep(Duration::from_millis(10));
        queue.cancel(first);
        // A busy machine can keep this thread from cancelling until after
        // the task fell due, and then the parked thread rightly returns at
        // its deadline. Only a cancel that ends before it moves the deadline.
        let cancelled_in_time = Instant::now() < due;
        let returned = thread.join().unwrap();
        assert!(returned >= if cancelled_in_time { bound } else { due });
    }

    #[test]
    fn cancelled_deadlines_stay_bounded_by_the_waiting_tasks() {
        let queue = Queue::<()>::new();
        let far = Instant::now() + Duration::from_secs(3600);
        let kept: Vec<_> = (0..10).map(|_| queue.post_at(far, |()| {})).collect();
        for _ in 0..10_000 {
            let task = queue.post_at(far, |()| {});
            queue.cancel(task);
        }
        let state = queue.inner.state.lock();
        assert_eq!(state.timed.len(), kept.len());
        assert!(state.deadlines.len() <= 2 * kept.len() + CANCELLED_SLACK + 1);
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
