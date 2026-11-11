//! Ports `src/mln/actor` and `util/{run_loop,async_task,thread,thread_pool}`
//! (default platform). Std threads and channels, no async runtime.
//!
//! Port-only: a host with one thread (the browser) installs a
//! [`CooperativeScheduler`] at startup
//! ([`scheduler::install_cooperative`]) and runs its tasks on that thread.

pub mod actor;
pub mod actor_ref;
pub mod async_task;
pub mod cooperative_scheduler;
pub mod established_actor;
pub mod local_object;
pub mod mailbox;
pub mod platform_settings;
pub mod run_loop;
pub mod scheduler;
pub mod thread;
pub mod thread_pool;
pub mod timer;

pub use actor_ref::ActorRef;
pub use async_task::{AsyncTask, AsyncTaskSender};
pub use cooperative_scheduler::CooperativeScheduler;
pub use established_actor::{AspiringActor, EstablishedActor};
pub use local_object::LocalObject;
pub use mailbox::Mailbox;
pub use run_loop::{EnteredRunLoop, RunLoop, RunLoopHandle};
pub use scheduler::{Scheduler, SchedulerPtr, TaggedScheduler};
pub use thread::Thread;
pub use thread_pool::ThreadPool;
pub use timer::Timer;
