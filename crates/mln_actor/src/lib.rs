//! Actors, schedulers, run loops, timers, threads and thread pools, built on
//! std threads and channels with no async runtime.
//!
//! A host with one thread (the browser) installs a
//! [`CooperativeScheduler`] at startup
//! ([`scheduler::install_cooperative`]) and runs its tasks on that thread.

pub mod actor;
pub mod actor_ref;
pub mod async_task;
pub mod cooperative_scheduler;
mod established_actor;
pub mod local_object;
pub mod mailbox;
pub mod run_loop;
pub mod scheduler;
pub mod thread;
pub mod thread_pool;
pub mod timer;

pub use actor_ref::ActorRef;
pub use async_task::{AsyncTask, AsyncTaskSender};
pub use cooperative_scheduler::CooperativeScheduler;
pub use local_object::LocalObject;
pub use mailbox::Mailbox;
pub use run_loop::{EnteredRunLoop, RunLoop, RunLoopHandle};
pub use scheduler::{Scheduler, SchedulerPtr, TaggedScheduler};
pub use thread::Thread;
pub use thread_pool::ThreadPool;
pub use timer::Timer;
