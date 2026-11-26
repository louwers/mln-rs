// A per-test wall-clock limit for the runner, so that one test that never
// finishes (a tile waiting forever for a reply, a request loop) errors with
// "timed out" instead of blocking the whole run.
//
// The map runs on the runner's thread and is not `Send`, so a hung test
// cannot be stopped from outside. Instead arming the watchdog posts a task
// to the test thread's queue for the limit's deadline; the task panics with
// `TestTimedOut` the next time the queue runs after it (every wait in the
// runner and the frontend runs the queue, and parks no longer than its next
// deadline), and the runner's `catch_unwind` around the test turns that into
// an errored test and drops the map. A test that hangs without running the
// queue is not caught.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mln_engine::exec::queue::Queue;

/// The panic payload of a test that ran out of time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TestTimedOut {
    pub(crate) limit: Duration,
}

impl TestTimedOut {
    #[must_use]
    pub(crate) fn message(&self) -> String {
        format!("Test timed out after {} s (--timeout)", self.limit.as_secs_f64())
    }
}

/// The per-test limit.
pub(crate) struct Watchdog {
    limit: Duration,
    /// The generation of the test that is running; a task posted for an
    /// earlier generation does nothing.
    current: Arc<AtomicU64>,
}

impl Watchdog {
    #[must_use]
    pub(crate) fn new(limit: Duration) -> Self {
        Watchdog { limit, current: Arc::new(AtomicU64::new(0)) }
    }

    /// Starts the limit for the test about to run on `queue`, this
    /// thread's; the returned guard stops it.
    #[must_use]
    pub(crate) fn arm(&self, queue: &Queue<()>) -> WatchdogGuard<'_> {
        let generation = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        let current = Arc::clone(&self.current);
        let payload = TestTimedOut { limit: self.limit };
        queue.post_after(self.limit, move |()| {
            if current.load(Ordering::SeqCst) == generation {
                // resume_unwind: no panic-hook message; the runner reports it.
                std::panic::resume_unwind(Box::new(payload));
            }
        });
        WatchdogGuard { watchdog: self }
    }

    fn disarm(&self) {
        // The task posted for this test sees a newer generation.
        self.current.fetch_add(1, Ordering::SeqCst);
    }
}

/// Stops the limit when dropped.
pub(crate) struct WatchdogGuard<'a> {
    watchdog: &'a Watchdog,
}

impl Drop for WatchdogGuard<'_> {
    fn drop(&mut self) {
        self.watchdog.disarm();
    }
}
