// libuv is replaced by a plain loop: `run_once` is `uv_run(UV_RUN_NOWAIT)`,
// i.e. it runs every async handle that was signalled, in registration order
// (the loop's own task queue first, as its handle is created in the RunLoop
// constructor), then returns without blocking. `run` is
// `uv_run(UV_RUN_DEFAULT)` with the holder handle ref'd: it waits for a
// wake-up (a task from any thread, or an AsyncTask send), runs once, and
// returns once a `stop()` task ran (C++ `stop` invokes a task that unrefs
// the holder handle).
//
// The task queues live in a `Send + Sync` core (`RunLoopHandle`), which
// mailboxes and other threads post to, as they post through a C++
// `RunLoop*`. The async handles stay on the loop's thread.
//
// Unlike C++, a thread has no current loop: what runs on a loop is given the
// loop, or its handle, by whoever made it.
//
// Timers (`util::Timer`, timer.rs) run first in `run_once`, as `uv_run` runs
// due timers before the poll phase; a blocking `run()` sleeps until the next
// timer is due. Timers do not keep `run()` alive (libuv unrefs them).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak as WeakArc};

use parking_lot::{Condvar, Mutex};

use crate::timer::Timers;

type Task = Box<dyn FnOnce() + Send + 'static>;

/// The thread-safe part of a `RunLoop`: the task queue (`defaultQueue` and
/// its mutex in C++), the pending flag of the loop's own async handle, and
/// the wake-up a blocking `run()` waits on.
pub(crate) struct Core {
    queue: Mutex<VecDeque<Task>>,
    /// The `RunLoop`'s own async handle (`impl->async`), which runs `process`.
    queue_pending: AtomicBool,
    /// Set by an `AsyncTask` send since `run_once` last looked at the
    /// handles, for `has_pending`.
    pub(crate) async_pending: AtomicBool,
    /// Set by every wake-up; `run()` sleeps until it is set.
    signal: Mutex<bool>,
    condvar: Condvar,
    /// The holder handle is unref'd (`stop()` ran).
    stopped: AtomicBool,
    /// Told about every wake-up, for a loop that a host drives
    /// with `run_once` instead of `run` (see `RunLoop::set_waker`).
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Core {
    fn new() -> Self {
        Core {
            queue: Mutex::new(VecDeque::new()),
            queue_pending: AtomicBool::new(false),
            async_pending: AtomicBool::new(false),
            signal: Mutex::new(false),
            condvar: Condvar::new(),
            stopped: AtomicBool::new(false),
            waker: Mutex::new(None),
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
    handles: RefCell<Vec<Weak<AsyncHandle>>>,
    /// The buffer that `run_once` copies the handles into, kept between
    /// passes so that a pass doesn't allocate.
    sending: Cell<Vec<Rc<AsyncHandle>>>,
    timers: Rc<Timers>,
}

/// A thread's event loop: its tasks, async tasks and timers. Cheap to clone:
/// clones refer to the same loop.
#[derive(Clone)]
pub struct RunLoop {
    inner: Rc<Inner>,
}

/// A `Send + Sync` reference to a `RunLoop`'s task queue (what other threads
/// hold as a `RunLoop*` in C++): `invoke` and `stop`.
#[derive(Clone)]
pub struct RunLoopHandle {
    core: Arc<Core>,
}

/// A [`RunLoopHandle`] that doesn't keep the loop's queue alive.
#[derive(Clone)]
pub struct WeakRunLoopHandle {
    core: WeakArc<Core>,
}

impl RunLoop {
    /// A loop for this thread.
    #[must_use]
    pub fn new() -> Self {
        RunLoop {
            inner: Rc::new(Inner {
                core: Arc::new(Core::new()),
                handles: RefCell::new(Vec::new()),
                sending: Cell::new(Vec::new()),
                timers: Rc::default(),
            }),
        }
    }

    /// A handle other threads can post to: mailboxes and requests that run
    /// their work on this loop take it.
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

    /// Whether a task or an `AsyncTask` send is waiting for `run_once`. Timers
    /// don't count.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        let core = &self.inner.core;
        core.queue_pending.load(Ordering::SeqCst) || core.async_pending.load(Ordering::SeqCst)
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
        // Every send sets `async_pending` after its handle's flag, so with no
        // send since the last pass, no handle is pending.
        if !self.inner.core.async_pending.swap(false, Ordering::SeqCst) {
            return did_work;
        }
        // Snapshot: callbacks may create or drop async handles. A pass from a
        // callback finds the buffer taken, and fills one of its own.
        let mut handles = self.inner.sending.take();
        {
            let mut registered = self.inner.handles.borrow_mut();
            registered.retain(|h| h.strong_count() > 0);
            handles.extend(registered.iter().filter_map(Weak::upgrade));
        }
        for handle in &handles {
            if !handle.closed.get() && handle.pending.swap(false, Ordering::SeqCst) {
                (handle.task.borrow_mut())();
                did_work = true;
            }
        }
        handles.clear();
        self.inner.sending.set(handles);
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
        let due = self.inner.timers.take_due(now);
        let any_due = !due.is_empty();
        for (timer, armed) in due {
            timer.fire(armed, now);
        }
        any_due
    }

    /// The earliest due time of this loop's running timers.
    fn next_timer_due(&self) -> Option<mln_util::time::Instant> {
        self.inner.timers.next_due()
    }

    pub(crate) fn timers(&self) -> &Rc<Timers> {
        &self.inner.timers
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

impl RunLoopHandle {
    /// [`RunLoop::set_waker`] from any thread.
    pub fn set_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.core.waker.lock() = waker;
    }

    /// Invokes `task` on the loop, from any thread.
    pub fn invoke(&self, task: impl FnOnce() + Send + 'static) {
        self.core.push(Box::new(task));
    }

    /// A handle that doesn't keep the loop's queue alive.
    #[must_use]
    pub fn downgrade(&self) -> WeakRunLoopHandle {
        WeakRunLoopHandle { core: Arc::downgrade(&self.core) }
    }

    /// Wakes the thread that waits for this loop's work
    /// ([`RunLoop::wait_for_work`]), from any thread, as a task would.
    pub fn wake(&self) {
        self.core.wake();
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

impl WeakRunLoopHandle {
    /// The handle, unless the loop's queue is gone.
    #[must_use]
    pub fn upgrade(&self) -> Option<RunLoopHandle> {
        self.core.upgrade().map(|core| RunLoopHandle { core })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn has_pending_sees_tasks_and_sends_until_they_run() {
        let run_loop = RunLoop::new();
        assert!(!run_loop.has_pending());
        run_loop.invoke(|| {});
        assert!(run_loop.has_pending());
        run_loop.run_once();
        assert!(!run_loop.has_pending());
        let task = crate::async_task::AsyncTask::new(&run_loop, || {});
        task.sender().send();
        assert!(run_loop.has_pending());
        run_loop.run_once();
        assert!(!run_loop.has_pending());
    }

    #[test]
    fn a_handle_that_a_callback_sends_runs_in_the_next_pass() {
        let run_loop = RunLoop::new();
        let ran = Rc::new(Cell::new(0));
        let created = Rc::new(RefCell::new(None));
        let (counter, slot, inner_loop) = (Rc::clone(&ran), Rc::clone(&created), run_loop.clone());
        let first = crate::AsyncTask::new(&run_loop, move || {
            let counter = Rc::clone(&counter);
            let task = crate::AsyncTask::new(&inner_loop, move || counter.set(counter.get() + 1));
            task.send();
            *slot.borrow_mut() = Some(task);
        });
        first.send();
        assert!(run_loop.run_once());
        assert_eq!(ran.get(), 0);
        assert!(run_loop.run_once());
        assert_eq!(ran.get(), 1);
        assert!(!run_loop.run_once());
    }

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
    fn a_loop_runs_only_what_its_own_scheduler_gets() {
        let first = RunLoop::new();
        let second = RunLoop::new();
        let mailbox = crate::mailbox::Mailbox::new();
        let received = Rc::new(Cell::new(false));
        mailbox.open(&second.handle(), Rc::<Cell<bool>>::clone(&received));
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
        assert!(!first.run_once());
        assert!(!received.get());
        while second.run_once() {}
        assert!(received.get());
        mailbox.close();
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
