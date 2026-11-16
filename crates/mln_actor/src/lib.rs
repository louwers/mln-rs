//! Actors, run loops, timers and threads, built on std threads and channels
//! with no async runtime. Background work runs on `mln_util::pool`.
//!
//! A host with one thread (the browser) installs a
//! [`CooperativeScheduler`] at startup
//! (`scheduler::install_cooperative`) and runs the threads' loops on that
//! thread.

mod actor_ref;
mod async_task;
pub mod cooperative_scheduler;
mod established_actor;
mod local_object;
mod mailbox;
mod run_loop;
pub mod scheduler;
mod thread;
pub mod timer;

pub use actor_ref::ActorRef;
pub use async_task::{AsyncTask, AsyncTaskSender};
pub use cooperative_scheduler::CooperativeScheduler;
pub use local_object::LocalObject;
pub use mailbox::Mailbox;
pub use run_loop::{RunLoop, RunLoopHandle, WeakRunLoopHandle};
pub use thread::Thread;
pub use timer::Timer;

#[cfg(test)]
mod tests;
