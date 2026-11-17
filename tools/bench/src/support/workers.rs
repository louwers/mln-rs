//! The background workers of a scenario's map, which can tell when they
//! have nothing left to do.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mln_actor::scheduler::{self, Scheduler, SchedulerPtr, Task};
use mln_util::identity::SimpleIdentity;
use parking_lot::{Condvar, Mutex};

/// How long [`Workers::wait_until_idle`] may wait before it fails the
/// scenario: a guard against a hang, not part of what runs.
const IDLE_LIMIT: Duration = Duration::from_secs(120);

/// The process's thread pool, behind a scheduler that counts the tasks
/// given to it and not yet finished.
///
/// A map, its renderer and its file source given these workers run their
/// background work (file reads, tile layout, glyph and sprite parsing) here,
/// so a scenario can wait until all of it is done and its replies are on
/// the map's run loop: a frame drawn then has everything it asked for
/// before, however fast the threads ran.
pub(crate) struct Workers {
    /// Taken at the first task, so that the pool's threads start inside the
    /// scenario's measured part when the map's first work does.
    pool: OnceLock<SchedulerPtr>,
    pending: Arc<(Mutex<usize>, Condvar)>,
    started: AtomicU64,
}

impl Workers {
    pub(crate) fn new() -> Arc<Workers> {
        Arc::new(Workers { pool: OnceLock::new(), pending: Arc::default(), started: AtomicU64::new(0) })
    }

    fn pool(&self) -> &SchedulerPtr {
        self.pool.get_or_init(scheduler::get_background)
    }

    /// The tasks given to the workers so far.
    pub(crate) fn started(&self) -> u64 {
        self.started.load(Ordering::Relaxed)
    }

    /// Whether no task is queued or running.
    pub(crate) fn is_idle(&self) -> bool {
        *self.pending.0.lock() == 0
    }

    /// Blocks until no task is queued or running. Panics after
    /// [`IDLE_LIMIT`].
    pub(crate) fn wait_until_idle(&self) {
        let (count, idle) = &*self.pending;
        let mut count = count.lock();
        idle.wait_while_for(&mut count, |count| *count != 0, IDLE_LIMIT);
        assert_eq!(*count, 0, "the workers are still busy after {IDLE_LIMIT:?}");
    }

    /// `task`, counted from now until it returns or unwinds.
    fn counted(&self, task: Task) -> Task {
        /// Counts the task done when dropped, after the task and what it
        /// captured.
        struct Done(Arc<(Mutex<usize>, Condvar)>);
        impl Drop for Done {
            fn drop(&mut self) {
                let (count, idle) = &*self.0;
                let mut count = count.lock();
                *count -= 1;
                if *count == 0 {
                    idle.notify_all();
                }
            }
        }
        self.started.fetch_add(1, Ordering::Relaxed);
        *self.pending.0.lock() += 1;
        let done = Done(Arc::clone(&self.pending));
        Box::new(move || {
            task();
            drop(done);
        })
    }
}

impl Scheduler for Workers {
    fn schedule(&self, task: Task) {
        let task = self.counted(task);
        self.pool().schedule(task);
    }

    fn schedule_tagged(&self, tag: SimpleIdentity, task: Task) {
        let task = self.counted(task);
        self.pool().schedule_tagged(tag, task);
    }

    fn wait_for_empty(&self) {
        self.pool().wait_for_empty();
    }

    fn wait_for_empty_tagged(&self, tag: SimpleIdentity) {
        self.pool().wait_for_empty_tagged(tag);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn idle_once_every_task_returned() {
        let workers = Workers::new();
        assert!(workers.is_idle());
        let ran = Arc::new(AtomicUsize::new(0));
        for _ in 0..16 {
            let ran = Arc::clone(&ran);
            workers.schedule_tagged(
                SimpleIdentity::new(),
                Box::new(move || {
                    std::thread::sleep(Duration::from_millis(1));
                    ran.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }
        workers.wait_until_idle();
        assert!(workers.is_idle());
        assert_eq!(ran.load(Ordering::SeqCst), 16);
        assert_eq!(workers.started(), 16);
    }
}
