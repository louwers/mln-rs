// A per-test wall-clock limit for the runner, so that one test that never
// finishes (a tile waiting forever for a reply, a request loop) errors with
// "timed out" instead of blocking the whole run.
//
// The map runs on the runtime's threads, and the runner waits for its
// results. Arming the watchdog sets a deadline for the runner's thread; each
// wait of the runner (`recv`, `sleep`) waits no longer than it, and panics
// with `TestTimedOut` once it passes. The runner's `catch_unwind` around the
// test turns that into an errored test and drops the map.

use std::cell::Cell;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

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

thread_local! {
    /// The deadline of the test that this thread runs, and its limit.
    static DEADLINE: Cell<Option<(Instant, Duration)>> = const { Cell::new(None) };
}

/// The per-test limit.
pub(crate) struct Watchdog {
    limit: Duration,
}

impl Watchdog {
    #[must_use]
    pub(crate) fn new(limit: Duration) -> Self {
        Watchdog { limit }
    }

    /// Starts the limit for the test about to run on this thread; the
    /// returned guard stops it.
    #[must_use]
    pub(crate) fn arm(&self) -> WatchdogGuard {
        DEADLINE.set(Some((Instant::now() + self.limit, self.limit)));
        WatchdogGuard(())
    }
}

/// Stops the limit when dropped.
pub(crate) struct WatchdogGuard(());

impl Drop for WatchdogGuard {
    fn drop(&mut self) {
        DEADLINE.set(None);
    }
}

/// How long the test has left, or `None` without a limit.
pub(crate) fn remaining() -> Option<Duration> {
    DEADLINE.get().map(|(at, _)| at.saturating_duration_since(Instant::now()))
}

/// Ends the test if its deadline passed.
pub(crate) fn check_deadline() {
    check(DEADLINE.get());
}

/// Ends the test if its deadline passed.
fn check(deadline: Option<(Instant, Duration)>) {
    if let Some((at, limit)) = deadline
        && Instant::now() >= at
    {
        // resume_unwind: no panic-hook message; the runner reports it.
        std::panic::resume_unwind(Box::new(TestTimedOut { limit }));
    }
}

/// The next value from `receiver`, or `None` once its senders are gone.
/// Panics with [`TestTimedOut`] when the test's deadline passes first.
pub(crate) fn recv<T>(receiver: &Receiver<T>) -> Option<T> {
    let Some((at, _)) = DEADLINE.get() else {
        return receiver.recv().ok();
    };
    loop {
        match receiver.recv_timeout(at.saturating_duration_since(Instant::now())) {
            Ok(value) => return Some(value),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => check(DEADLINE.get()),
        }
    }
}

/// Sleeps for `duration`. Panics with [`TestTimedOut`] when the test's
/// deadline passes first.
pub(crate) fn sleep(duration: Duration) {
    let deadline = DEADLINE.get();
    let end = Instant::now() + duration;
    match deadline {
        Some((at, _)) if at < end => {
            std::thread::sleep(at.saturating_duration_since(Instant::now()));
            check(deadline);
        }
        _ => std::thread::sleep(duration),
    }
}
