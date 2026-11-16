// Port-only: a host without threads installs a `CooperativeScheduler` at
// startup (`install_cooperative`), which runs the objects of `Thread`s on the
// host's thread (cooperative_scheduler.rs).

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use mln_util::pool::TaskGroup;

use crate::cooperative_scheduler::CooperativeScheduler;
use crate::local_object::LocalObject;
use crate::run_loop::RunLoopHandle;

static COOPERATIVE: OnceLock<Arc<CooperativeScheduler>> = OnceLock::new();

/// Port-only: makes `scheduler` run the objects of `util::Thread`s, which
/// stay on the creating thread's `RunLoop` (see `cooperative_scheduler.rs`).
/// For a host with one thread (the browser), which runs them with
/// `CooperativeScheduler::run_loops`. It must come before anything starts a
/// `Thread`, and only once.
pub fn install_cooperative(scheduler: Arc<CooperativeScheduler>) {
    if COOPERATIVE.set(scheduler).is_err() {
        panic!("a cooperative scheduler is already installed");
    }
}

/// Port-only: the installed cooperative scheduler, after installing a new
/// one if none is: `install_cooperative` for hosts that may each try to
/// install it, such as libraries that share a wasm instance. The first
/// install must still come before anything starts a `Thread`.
pub fn install_cooperative_once() -> &'static Arc<CooperativeScheduler> {
    COOPERATIVE.get_or_init(|| Arc::new(CooperativeScheduler::new()))
}

/// Port-only: the scheduler `install_cooperative` installed, if any.
pub fn cooperative() -> Option<&'static Arc<CooperativeScheduler>> {
    COOPERATIVE.get()
}

/// Where a task of `schedule_and_reply_later` hands its result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

/// Runs `task` in `group` with a `Respond` to call once with its result,
/// then or later and from any thread, and `reply(result)` on `reply_to`,
/// which runs its tasks on the calling thread. A `Respond` dropped without a
/// call drops the reply. Once the loop is gone, the task doesn't run.
pub fn schedule_and_reply_later<R, TaskFn, ReplyFn>(
    group: &TaskGroup,
    reply_to: &RunLoopHandle,
    task: TaskFn,
    reply: ReplyFn,
) where
    R: Send + 'static,
    TaskFn: FnOnce(Respond<R>) + Send + 'static,
    ReplyFn: FnOnce(R) + 'static,
{
    let reply_to = reply_to.downgrade();
    let reply = LocalObject::new(RefCell::new(Some(reply)));
    group.spawn(move || {
        let Some(reply_to) = reply_to.upgrade() else { return };
        task(Box::new(move |result| {
            reply_to.invoke(move || {
                if let Some(reply) = reply.take()
                    && let Some(reply) = reply.borrow_mut().take()
                {
                    reply(result);
                }
            });
        }));
    });
}
