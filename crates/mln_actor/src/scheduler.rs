// Ports include/mln/actor/scheduler.hpp and src/mln/actor/scheduler.cpp:
// the `Scheduler` interface, `GetCurrent`/`SetCurrent`, `GetBackground`,
// `bindOnce`, `scheduleAndReplyValue` and `TaggedScheduler`.
//
// `mapbox::base::WeakPtr<Scheduler>` is `Weak<dyn Scheduler + Send + Sync>`:
// schedulers that other threads post to live in an `Arc` (a RunLoop's
// `RunLoopHandle`, a `ThreadPool`).
//
// Port-only: a host without threads installs a `CooperativeScheduler` at
// startup (`install_cooperative`), which `GetBackground` and `GetSequenced`
// then return (cooperative_scheduler.rs).
//
// TODO(port): `runOnRenderThread`/`runRenderJobs`. (gtests-core added
// `GetSequenced` and `setExceptionHandler`.)

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use mln_util::identity::SimpleIdentity;
use mln_util::sync::MutexExt;

use crate::cooperative_scheduler::CooperativeScheduler;
use crate::local_object::LocalObject;
use crate::thread_pool::{SequencedScheduler, ThreadPool};

pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// `std::function<void(const std::exception_ptr)>`: gets the payload of a
/// task that panicked (gtests-core).
pub type ExceptionHandler = Box<dyn Fn(Box<dyn std::any::Any + Send>) + Send + Sync>;

/// A `Scheduler` is responsible for coordinating the processing of messages
/// by one or more actors via their mailboxes.
pub trait Scheduler {
    /// Enqueues a function for execution.
    fn schedule(&self, task: Task);

    /// Enqueues a function for execution in the queue of `tag` (schedulers
    /// without tagged queues ignore the tag, as the C++ `RunLoop` does).
    fn schedule_tagged(&self, _tag: SimpleIdentity, task: Task) {
        self.schedule(task);
    }

    /// Wait until there's nothing pending or in process. Must not be called
    /// from a task provided to this scheduler.
    fn wait_for_empty(&self);

    /// `waitForEmpty(tag)`.
    fn wait_for_empty_tagged(&self, _tag: SimpleIdentity) {
        self.wait_for_empty();
    }

    /// `setExceptionHandler(handler)` (gtests-core): called when a task on a
    /// thread this scheduler controls panics (only thread pools call it, as
    /// in C++).
    fn set_exception_handler(&self, _handler: ExceptionHandler) {}
}

pub type SchedulerPtr = Arc<dyn Scheduler + Send + Sync>;
pub type WeakScheduler = Weak<dyn Scheduler + Send + Sync>;

thread_local! {
    static LOCAL_SCHEDULER: RefCell<Option<SchedulerPtr>> = const { RefCell::new(None) };
}

/// `Scheduler::SetCurrent`.
pub fn set_current(scheduler: Option<SchedulerPtr>) {
    LOCAL_SCHEDULER.with(|s| *s.borrow_mut() = scheduler);
}

/// `Scheduler::GetCurrent(init = true)`: creates the thread's `RunLoop` if it
/// has no scheduler yet.
#[must_use]
pub fn get_current() -> SchedulerPtr {
    if let Some(scheduler) = try_get_current() {
        return scheduler;
    }
    // `static thread_local util::RunLoop runLoop; SetCurrent(&runLoop);`
    let _ = crate::run_loop::RunLoop::get();
    try_get_current().expect("RunLoop sets the current scheduler")
}

/// `Scheduler::GetCurrent(false)`.
#[must_use]
pub fn try_get_current() -> Option<SchedulerPtr> {
    LOCAL_SCHEDULER.with(|s| s.borrow().clone())
}

static COOPERATIVE: OnceLock<Arc<CooperativeScheduler>> = OnceLock::new();

/// Port-only: makes `scheduler` run everything that would otherwise run on
/// threads of its own: the background and sequenced schedulers, and the
/// objects of `util::Thread`s, which stay on the creating thread's `RunLoop`
/// (see `cooperative_scheduler.rs`). For a host with one thread (the
/// browser), which runs the tasks with `CooperativeScheduler::run_until`.
/// It must come before anything gets a scheduler or starts a `Thread`, and
/// only once.
pub fn install_cooperative(scheduler: Arc<CooperativeScheduler>) {
    if COOPERATIVE.set(scheduler).is_err() {
        panic!("a cooperative scheduler is already installed");
    }
}

/// Port-only: the scheduler `install_cooperative` installed, if any.
pub fn cooperative() -> Option<&'static Arc<CooperativeScheduler>> {
    COOPERATIVE.get()
}

/// `Scheduler::GetBackground()`: the process-wide thread pool, created on
/// first use and kept alive by its users (or the installed cooperative
/// scheduler).
pub fn get_background() -> SchedulerPtr {
    static WEAK: Mutex<Option<Weak<ThreadPool>>> = Mutex::new(None);
    if let Some(cooperative) = cooperative() {
        return cooperative.clone();
    }
    let mut weak = WEAK.lock_unpoisoned();
    if let Some(scheduler) = weak.as_ref().and_then(Weak::upgrade) {
        return scheduler;
    }
    let scheduler = Arc::new(ThreadPool::new());
    *weak = Some(Arc::downgrade(&scheduler));
    scheduler
}

/// `Scheduler::GetSequenced()` (gtests-core): one of 10 process-wide
/// single-thread schedulers, round robin, each created on first use and
/// kept alive by its users (or the installed cooperative scheduler).
pub fn get_sequenced() -> SchedulerPtr {
    const SCHEDULERS_COUNT: usize = 10;
    struct State {
        weaks: Vec<Option<Weak<SequencedScheduler>>>,
        last_used_index: usize,
    }
    static STATE: Mutex<State> = Mutex::new(State { weaks: Vec::new(), last_used_index: 0 });
    if let Some(cooperative) = cooperative() {
        return cooperative.clone();
    }
    let mut state = STATE.lock_unpoisoned();
    if state.weaks.is_empty() {
        state.weaks.resize(SCHEDULERS_COUNT, None);
    }
    state.last_used_index = (state.last_used_index + 1) % SCHEDULERS_COUNT;
    let index = state.last_used_index;
    if let Some(scheduler) = state.weaks[index].as_ref().and_then(Weak::upgrade) {
        return scheduler;
    }
    let result = Arc::new(SequencedScheduler::new());
    state.weaks[index] = Some(Arc::downgrade(&result));
    result
}

/// `Scheduler::bindOnce(fn)`: a function that, when called, schedules `fn`
/// on `scheduler` (if it is still alive). Repeated calls do nothing.
pub fn bind_once(scheduler: &SchedulerPtr, task: Task) -> impl FnMut() + Send + 'static {
    let scheduler = Arc::downgrade(scheduler);
    let mut scheduled = Some(task);
    move || {
        let Some(task) = scheduled.take() else {
            return; // Repeated call.
        };
        if let Some(scheduler) = scheduler.upgrade() {
            scheduler.schedule(task);
        }
    }
}

/// `scheduleAndReplyValue(tag, task, reply)`: runs `task` on `scheduler`
/// and then `reply(result)` on the current thread's scheduler.
///
/// C++ captures `reply` in the closure that travels through the pool; here
/// it stays on the calling thread (it usually holds thread-local state such
/// as `Rc`s, which C++ reaches through raw pointers) and the result is
/// handed back to it.
pub fn schedule_and_reply_value<R, TaskFn, ReplyFn>(
    scheduler: &dyn Scheduler,
    tag: SimpleIdentity,
    task: TaskFn,
    reply: ReplyFn,
) where
    R: Send + 'static,
    TaskFn: FnOnce() -> R + Send + 'static,
    ReplyFn: FnOnce(R) + 'static,
{
    schedule_and_reply_later(scheduler, tag, move |respond: Respond<R>| respond(task()), reply);
}

/// Where a task of `schedule_and_reply_later` hands its result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

/// Port-only: `scheduleAndReplyValue` for a task whose result may come
/// after it returns (an image the browser decodes): `task` runs on
/// `scheduler` with a `Respond` to call once with its result, then or later
/// and from any thread, and `reply(result)` runs on the current thread's
/// scheduler. A `Respond` dropped without a call drops the reply.
pub fn schedule_and_reply_later<R, TaskFn, ReplyFn>(
    scheduler: &dyn Scheduler,
    tag: SimpleIdentity,
    task: TaskFn,
    reply: ReplyFn,
) where
    R: Send + 'static,
    TaskFn: FnOnce(Respond<R>) + Send + 'static,
    ReplyFn: FnOnce(R) + 'static,
{
    let reply_scheduler = Arc::downgrade(&get_current());
    let reply = LocalObject::new(RefCell::new(Some(reply)));
    scheduler.schedule_tagged(
        tag,
        Box::new(move || {
            if let Some(reply_scheduler) = reply_scheduler.upgrade() {
                task(Box::new(move |result| {
                    reply_scheduler.schedule_tagged(
                        tag,
                        Box::new(move || {
                            if let Some(reply) = reply.take()
                                && let Some(reply) = reply.borrow_mut().take()
                            {
                                reply(result);
                            }
                        }),
                    );
                }));
            }
        }),
    );
}

/// `TaggedScheduler`: a scheduler plus the tag its tasks are queued under.
#[derive(Clone)]
pub struct TaggedScheduler {
    pub tag: SimpleIdentity,
    scheduler: SchedulerPtr,
}

impl TaggedScheduler {
    pub fn new(scheduler: SchedulerPtr, tag: SimpleIdentity) -> Self {
        TaggedScheduler { tag, scheduler }
    }

    #[must_use]
    pub fn get(&self) -> &SchedulerPtr {
        &self.scheduler
    }

    pub fn schedule(&self, task: Task) {
        self.scheduler.schedule_tagged(self.tag, task);
    }

    pub fn wait_for_empty(&self) {
        self.scheduler.wait_for_empty_tagged(self.tag);
    }

    pub fn schedule_and_reply_value<R, TaskFn, ReplyFn>(&self, task: TaskFn, reply: ReplyFn)
    where
        R: Send + 'static,
        TaskFn: FnOnce() -> R + Send + 'static,
        ReplyFn: FnOnce(R) + 'static,
    {
        schedule_and_reply_value(&*self.scheduler, self.tag, task, reply);
    }

    /// `schedule_and_reply_later` on this scheduler and tag.
    pub fn schedule_and_reply_later<R, TaskFn, ReplyFn>(&self, task: TaskFn, reply: ReplyFn)
    where
        R: Send + 'static,
        TaskFn: FnOnce(Respond<R>) + Send + 'static,
        ReplyFn: FnOnce(R) + 'static,
    {
        schedule_and_reply_later(&*self.scheduler, self.tag, task, reply);
    }
}
