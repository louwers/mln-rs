// libuv is replaced by a plain loop: `run_once` is `uv_run(UV_RUN_NOWAIT)`,
// i.e. it runs every async handle that was signalled, in registration order
// (the loop's own task queue first, as its handle is created in the RunLoop
// constructor), then returns without blocking. `run` is
// `uv_run(UV_RUN_DEFAULT)` with the holder handle ref'd: it waits for a
// wake-up (a task from any thread, or an AsyncTask send), runs once, and
// returns once a `stop()` task ran (C++ `stop` invokes a task that unrefs
// the holder handle).
//
// The task queues live in a `Send + Sync` core (`RunLoopHandle`), which is
// also the loop's `Scheduler` for mailboxes and `Scheduler::get_current()`:
// other threads post to it, as they post through a C++ `RunLoop*`. The async
// handles stay on the loop's thread.
//
// Timers (`util::Timer`, timer.rs) run first in `run_once`, as `uv_run` runs
// due timers before the poll phase; a blocking `run()` sleeps until the next
// timer is due. Timers do not keep `run()` alive (libuv unrefs them).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Condvar, Mutex};

use crate::scheduler::{self, Scheduler};
use crate::timer::TimerHandle;

type Task = Box<dyn FnOnce() + Send + 'static>;

/// The thread-safe part of a `RunLoop`: the task queue (`defaultQueue` and
/// its mutex in C++), the pending flag of the loop's own async handle, and
/// the wake-up a blocking `run()` waits on.
pub(crate) struct Core {
    queue: Mutex<VecDeque<Task>>,
    /// The `RunLoop`'s own async handle (`impl->async`), which runs `process`.
    queue_pending: AtomicBool,
    /// Set by every wake-up; `run()` sleeps until it is set.
    signal: Mutex<bool>,
    condvar: Condvar,
    /// The holder handle is unref'd (`stop()` ran).
    stopped: AtomicBool,
    /// Told about every wake-up, for a loop that a host drives
    /// with `run_once` instead of `run` (see `RunLoop::set_waker`).
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Whether a `RunLoop` runs this queue (see `RunLoop::adopt`).
    adopted: AtomicBool,
}

impl Core {
    fn new() -> Self {
        Core {
            queue: Mutex::new(VecDeque::new()),
            queue_pending: AtomicBool::new(false),
            signal: Mutex::new(false),
            condvar: Condvar::new(),
            stopped: AtomicBool::new(false),
            waker: Mutex::new(None),
            adopted: AtomicBool::new(false),
        }
    }

    fn push(&self, task: Task) {
        self.queue.lock().push_back(task);
        // wake()
        self.queue_pending.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Wakes a blocking `run()` (what `uv_async_send` does for libuv).
    pub(crate) fn wake(&self) {
        {
            let mut signalled = self.signal.lock();
            *signalled = true;
            self.condvar.notify_all();
        }
        let waker = self.waker.lock().clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    fn wait(&self) {
        let mut signalled = self.signal.lock();
        while !*signalled {
            self.condvar.wait(&mut signalled);
        }
        *signalled = false;
    }

    /// `wait`, but returns at `deadline` at the latest.
    fn wait_until(&self, deadline: mln_util::time::Instant) {
        let mut signalled = self.signal.lock();
        while !*signalled {
            let now = mln_util::time::Instant::now();
            if now >= deadline {
                return;
            }
            self.condvar.wait_for(&mut signalled, deadline - now);
        }
        *signalled = false;
    }

    fn process(&self) {
        loop {
            let Some(task) = self.queue.lock().pop_front() else {
                break;
            };
            task();
        }
    }

    fn remaining(&self) -> usize {
        self.queue.lock().len()
    }
}

/// An async handle (`uv_async_t`): a coalescing "pending" flag plus the
/// callback that runs on the loop's thread.
pub(crate) struct AsyncHandle {
    pub(crate) pending: Arc<AtomicBool>,
    /// gtests-core: set when the `AsyncTask` is destroyed (`uv_close`): a
    /// signalled task that another callback of the same `run_once` destroys
    /// must not fire (`test/util/async_task.test.cpp` `DestroyAfterSignaling`).
    pub(crate) closed: Cell<bool>,
    pub(crate) task: RefCell<Box<dyn FnMut()>>,
    pub(crate) core: Arc<Core>,
}

struct Inner {
    core: Arc<Core>,
    /// The loop's scheduler, one for its life: mailboxes and actors hold
    /// their scheduler weakly, so one made each time the loop is entered
    /// would go once its guard drops, with what they send to it.
    scheduler: scheduler::SchedulerPtr,
    handles: RefCell<Vec<Weak<AsyncHandle>>>,
    timers: RefCell<Vec<Weak<TimerHandle>>>,
}

thread_local! {
    static CURRENT: RefCell<Option<Rc<Inner>>> = const { RefCell::new(None) };
}

/// Makes a loop the current loop and scheduler of its thread until it drops,
/// then restores the ones before (see `RunLoop::enter`).
#[must_use = "the loop is current only until the guard drops"]
pub struct EnteredRunLoop {
    previous: Option<Rc<Inner>>,
    previous_scheduler: Option<scheduler::SchedulerPtr>,
}

impl Drop for EnteredRunLoop {
    fn drop(&mut self) {
        CURRENT.with(|c| *c.borrow_mut() = self.previous.take());
        scheduler::set_current(self.previous_scheduler.take());
    }
}

/// A thread's event loop. Cheap to clone: clones refer to the same loop.
#[derive(Clone)]
pub struct RunLoop {
    inner: Rc<Inner>,
}

/// A `Send + Sync` reference to a `RunLoop`'s task queue (what other threads
/// hold as a `RunLoop*` in C++): `invoke`, `stop`, and the loop's
/// `Scheduler`.
#[derive(Clone)]
pub struct RunLoopHandle {
    core: Arc<Core>,
}

impl RunLoop {
    /// Creates a loop and makes it the current scheduler of this thread.
    #[must_use]
    pub fn new() -> Self {
        let core = Arc::new(Core::new());
        core.adopted.store(true, Ordering::SeqCst);
        let run_loop = RunLoop::over(core);
        CURRENT.with(|c| *c.borrow_mut() = Some(Rc::clone(&run_loop.inner)));
        scheduler::set_current(Some(run_loop.scheduler()));
        run_loop
    }

    fn over(core: Arc<Core>) -> Self {
        let scheduler = Arc::new(RunLoopHandle { core: Arc::clone(&core) });
        RunLoop {
            inner: Rc::new(Inner {
                core,
                scheduler,
                handles: RefCell::new(Vec::new()),
                timers: RefCell::new(Vec::new()),
            }),
        }
    }

    fn scheduler(&self) -> scheduler::SchedulerPtr {
        Arc::clone(&self.inner.scheduler)
    }

    /// The loop that runs the queue `handle` (see
    /// [`RunLoopHandle::new`]) on this thread, with the tasks sent to it so
    /// far. `None` when a loop runs that queue already. Unlike `new`, it
    /// leaves this thread's current loop as it is: [`enter`](Self::enter)
    /// makes the loop current while it runs.
    #[must_use]
    pub fn adopt(handle: &RunLoopHandle) -> Option<RunLoop> {
        if handle.core.adopted.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(RunLoop::over(Arc::clone(&handle.core)))
    }

    /// Makes this loop the current loop and scheduler of this
    /// thread until the guard drops, which restores the ones before, so one
    /// thread can run several loops in turn. What runs meanwhile, such as
    /// a timer or a mailbox that a task creates, belongs to this loop.
    pub fn enter(&self) -> EnteredRunLoop {
        let previous = CURRENT.with(|c| c.borrow_mut().replace(Rc::clone(&self.inner)));
        let previous_scheduler = scheduler::try_get_current();
        scheduler::set_current(Some(self.scheduler()));
        EnteredRunLoop { previous, previous_scheduler }
    }

    /// Returns this thread's current `RunLoop`, creating one on first use.
    /// C++ asserts that the thread has one; creating it is what the C++ test
    /// harnesses do first thing (and what `get_current` does).
    #[must_use]
    pub fn get() -> RunLoop {
        let existing = CURRENT.with(|c| c.borrow().clone());
        match existing {
            Some(inner) => RunLoop { inner },
            None => RunLoop::new(),
        }
    }

    /// The current thread's loop, if it has one.
    #[must_use]
    pub(crate) fn try_get() -> Option<RunLoop> {
        CURRENT.with(|c| c.borrow().clone()).map(|inner| RunLoop { inner })
    }

    /// A handle other threads can post to.
    #[must_use]
    pub fn handle(&self) -> RunLoopHandle {
        RunLoopHandle { core: Arc::clone(&self.inner.core) }
    }

    /// Invoke `task` on this `RunLoop`.
    pub fn invoke(&self, task: impl FnOnce() + Send + 'static) {
        self.inner.core.push(Box::new(task));
    }

    /// Calls `waker`, from the waking thread, whenever this loop
    /// gets work (a task or an `AsyncTask` send), so that a host that drives
    /// the loop with `run_once` on its own thread knows to run it. Timers
    /// don't wake: such a host runs the loop at least once a frame.
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.inner.core.waker.lock() = waker;
    }

    /// Whether a [`stop`](Self::stop) task ran, for a host that drives the
    /// loop with `run_once` rather than `run`.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.inner.core.stopped.load(Ordering::SeqCst)
    }

    /// `uv_run(loop, UV_RUN_NOWAIT)`. Returns whether a timer, task or
    /// async handle ran.
    pub fn run_once(&self) -> bool {
        let mut did_work = self.run_timers();
        if self.inner.core.queue_pending.swap(false, Ordering::SeqCst) {
            self.inner.core.process();
            did_work = true;
        }
        // Snapshot: callbacks may create or drop async handles.
        let handles: Vec<Rc<AsyncHandle>> = {
            let mut handles = self.inner.handles.borrow_mut();
            handles.retain(|h| h.strong_count() > 0);
            handles.iter().filter_map(Weak::upgrade).collect()
        };
        for handle in handles {
            if !handle.closed.get() && handle.pending.swap(false, Ordering::SeqCst) {
                (handle.task.borrow_mut())();
                did_work = true;
            }
        }
        did_work
    }

    /// Runs the loop until `stop`. It blocks, so a host that runs on the
    /// browser's event loop doesn't call it: it drives the loop with
    /// `run_once` instead (see `set_waker`).
    pub fn run(&self) {
        let core = &self.inner.core;
        core.stopped.store(false, Ordering::SeqCst);
        loop {
            self.run_once();
            if core.stopped.load(Ordering::SeqCst) {
                return;
            }
            self.wait_for_work();
        }
    }

    /// Sleeps until this loop gets a task or an async handle send, or its
    /// next timer is due. It can return early, when a wake-up came while
    /// the loop was running.
    pub fn wait_for_work(&self) {
        let core = &self.inner.core;
        match self.next_timer_due() {
            Some(deadline) => core.wait_until(deadline),
            None => core.wait(),
        }
    }

    /// `uv__run_timers`: runs the timers that are due, earliest first (ties
    /// in start order). A callback may stop, restart or drop any timer.
    /// Returns whether any was due.
    fn run_timers(&self) -> bool {
        let now = mln_util::time::Instant::now();
        let mut due: Vec<Rc<TimerHandle>> = {
            let mut timers = self.inner.timers.borrow_mut();
            timers.retain(|t| t.strong_count() > 0);
            timers.iter().filter_map(Weak::upgrade).filter(|t| t.due.get().is_some_and(|d| d <= now)).collect()
        };
        due.sort_by_key(|t| (t.due.get(), t.start_id.get()));
        let any_due = !due.is_empty();
        for timer in due {
            // An earlier callback may have stopped or restarted it.
            if timer.due.get().is_some_and(|d| d <= now) {
                timer.fire(now);
            }
        }
        any_due
    }

    /// The earliest due time of this loop's running timers.
    fn next_timer_due(&self) -> Option<mln_util::time::Instant> {
        self.inner.timers.borrow().iter().filter_map(Weak::upgrade).filter_map(|t| t.due.get()).min()
    }

    pub(crate) fn register_timer(&self, timer: &Rc<TimerHandle>) {
        self.inner.timers.borrow_mut().push(Rc::downgrade(timer));
    }

    /// Makes `run` return once the tasks queued before this one have run.
    pub fn stop(&self) {
        self.handle().stop();
    }

    pub(crate) fn register(&self, handle: &Rc<AsyncHandle>) {
        self.inner.handles.borrow_mut().push(Rc::downgrade(handle));
    }

    pub(crate) fn core(&self) -> Arc<Core> {
        Arc::clone(&self.inner.core)
    }

    pub fn wait_for_empty(&self) {
        loop {
            if self.inner.core.remaining() == 0 {
                return;
            }
            self.run_once();
        }
    }
}

impl Default for RunLoop {
    fn default() -> Self {
        RunLoop::new()
    }
}

impl Scheduler for RunLoop {
    fn schedule(&self, task: Box<dyn FnOnce() + Send + 'static>) {
        self.inner.core.push(task);
    }

    fn wait_for_empty(&self) {
        RunLoop::wait_for_empty(self);
    }
}

impl Default for RunLoopHandle {
    fn default() -> Self {
        RunLoopHandle::new()
    }
}

impl RunLoopHandle {
    /// The task queue of a loop that no thread runs yet. Tasks
    /// sent to it wait until a thread makes the loop with
    /// [`RunLoop::adopt`] and runs it.
    #[must_use]
    pub fn new() -> RunLoopHandle {
        RunLoopHandle { core: Arc::new(Core::new()) }
    }

    /// [`RunLoop::set_waker`] from any thread.
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.core.waker.lock() = waker;
    }

    /// Invokes `task` on the loop, from any thread.
    pub fn invoke(&self, task: impl FnOnce() + Send + 'static) {
        self.core.push(Box::new(task));
    }

    pub fn stop(&self) {
        let core = Arc::downgrade(&self.core);
        self.invoke(move || {
            if let Some(core) = core.upgrade() {
                core.stopped.store(true, Ordering::SeqCst);
            }
        });
    }
}

impl Scheduler for RunLoopHandle {
    fn schedule(&self, task: Box<dyn FnOnce() + Send + 'static>) {
        self.core.push(task);
    }

    /// Must be called on the loop's own thread (as in C++, where it calls
    /// `runOnce`); elsewhere it waits for the queue to drain.
    fn wait_for_empty(&self) {
        if let Some(run_loop) = RunLoop::try_get()
            && Arc::ptr_eq(&run_loop.inner.core, &self.core)
        {
            run_loop.wait_for_empty();
            return;
        }
        while self.core.remaining() != 0 {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn invoke_runs_on_run_once() {
        let run_loop = RunLoop::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        run_loop.invoke(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 0);
        run_loop.run_once();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        run_loop.run_once();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_loop_run_once_at_a_time_sees_its_stop() {
        let run_loop = RunLoop::new();
        run_loop.stop();
        assert!(!run_loop.is_stopped());
        run_loop.run_once();
        assert!(run_loop.is_stopped());
    }

    #[test]
    fn an_adopted_loop_runs_what_was_sent_before() {
        let handle = RunLoopHandle::new();
        let (tx, rx) = std::sync::mpsc::channel();
        handle.invoke(move || tx.send(std::thread::current().id()).unwrap());
        let thread = std::thread::spawn(move || {
            let run_loop = RunLoop::adopt(&handle).unwrap();
            assert!(RunLoop::adopt(&handle).is_none());
            run_loop.run_once();
            std::thread::current().id()
        });
        let ran_on = thread.join().unwrap();
        assert_eq!(ran_on, rx.recv().unwrap());
    }

    #[test]
    fn a_mailbox_opened_in_an_entered_loop_receives_after_the_guard_drops() {
        let run_loop = RunLoop::adopt(&RunLoopHandle::new()).unwrap();
        let mailbox = crate::mailbox::Mailbox::new();
        let received = Rc::new(Cell::new(false));
        {
            let _entered = run_loop.enter();
            mailbox.open(&scheduler::get_current(), Rc::<Cell<bool>>::clone(&received));
        }
        // Another thread sends while the loop isn't entered, and the loop
        // delivers once it is again.
        let sender = Arc::clone(&mailbox);
        std::thread::spawn(move || {
            sender.push(Box::new(|target: &dyn std::any::Any| {
                if let Some(received) = target.downcast_ref::<Cell<bool>>() {
                    received.set(true);
                }
            }));
        })
        .join()
        .unwrap();
        let _entered = run_loop.enter();
        while run_loop.run_once() {}
        assert!(received.get());
        mailbox.close();
    }

    #[test]
    fn entering_a_loop_makes_it_current_until_the_guard_drops() {
        let outer = RunLoop::new();
        let inner = RunLoop::adopt(&RunLoopHandle::new()).unwrap();
        let current = || RunLoop::try_get().map(|run_loop| Arc::as_ptr(&run_loop.inner.core));
        assert_eq!(Some(Arc::as_ptr(&outer.inner.core)), current());
        {
            let _entered = inner.enter();
            assert_eq!(Some(Arc::as_ptr(&inner.inner.core)), current());
            // The ambient scheduler posts to the entered loop.
            let ran = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&ran);
            scheduler::get_current().schedule(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));
            inner.run_once();
            assert_eq!(1, ran.load(Ordering::SeqCst));
        }
        assert_eq!(Some(Arc::as_ptr(&outer.inner.core)), current());
    }

    #[test]
    fn run_until_stopped_from_another_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let thread = std::thread::spawn(move || {
            let run_loop = RunLoop::new();
            tx.send(run_loop.handle()).unwrap();
            run_loop.run();
            c.load(Ordering::SeqCst)
        });
        let handle = rx.recv().unwrap();
        for _ in 0..10 {
            let c = Arc::clone(&count);
            handle.invoke(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }
        handle.stop();
        assert_eq!(thread.join().unwrap(), 10);
    }
}
