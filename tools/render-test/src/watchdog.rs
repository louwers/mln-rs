// A per-test wall-clock limit for the runner, so that one test that never
// finishes (a tile waiting forever for a reply, a request loop) errors with
// "timed out" instead of blocking the whole run.
//
// The map runs on the runner's thread and is not `Send`, so a hung test
// cannot be stopped from outside. Instead the watchdog thread posts a task
// to the test thread's RunLoop when the limit passes; the task panics with
// `TestTimedOut` the next time the loop runs (every wait in the runner and
// the frontend spins `RunLoop::runOnce`), and the runner's `catch_unwind`
// around the test turns that into an errored test and drops the map. A
// test that hangs without running the loop is not caught.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mln_actor::{RunLoop, RunLoopHandle};

/// The panic payload of a test that ran out of time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestTimedOut {
    pub limit: Duration,
}

impl TestTimedOut {
    #[must_use]
    pub fn message(&self) -> String {
        format!("Test timed out after {} s (--timeout)", self.limit.as_secs_f64())
    }
}

struct Armed {
    deadline: Instant,
    generation: u64,
    limit: Duration,
    run_loop: RunLoopHandle,
}

#[derive(Default)]
struct State {
    armed: Option<Armed>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wakeup: Condvar,
    /// The generation of the test that is running; a task posted for an
    /// earlier generation does nothing.
    current: Arc<AtomicU64>,
}

/// The watchdog thread.
pub struct Watchdog {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    limit: Duration,
}

impl Watchdog {
    #[must_use]
    pub fn new(limit: Duration) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wakeup: Condvar::new(),
            current: Arc::new(AtomicU64::new(0)),
        });
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("render-test-watchdog".to_owned())
            .spawn(move || watch(&thread_shared))
            .expect("spawn the watchdog thread");
        Watchdog { shared, thread: Some(thread), limit }
    }

    /// Starts the limit for the test about to run on this thread; the
    /// returned guard stops it.
    #[must_use]
    pub fn arm(&self) -> WatchdogGuard<'_> {
        let generation = self.shared.current.fetch_add(1, Ordering::SeqCst) + 1;
        let mut state = self.shared.state.lock().unwrap();
        state.armed = Some(Armed {
            deadline: Instant::now() + self.limit,
            generation,
            limit: self.limit,
            run_loop: RunLoop::get().handle(),
        });
        self.shared.wakeup.notify_all();
        WatchdogGuard { watchdog: self }
    }

    fn disarm(&self) {
        // A task already posted for this test sees a newer generation.
        self.shared.current.fetch_add(1, Ordering::SeqCst);
        let mut state = self.shared.state.lock().unwrap();
        state.armed = None;
        self.shared.wakeup.notify_all();
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.shutdown = true;
            self.shared.wakeup.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Stops the limit when dropped.
pub struct WatchdogGuard<'a> {
    watchdog: &'a Watchdog,
}

impl Drop for WatchdogGuard<'_> {
    fn drop(&mut self) {
        self.watchdog.disarm();
    }
}

fn watch(shared: &Shared) {
    let mut state = shared.state.lock().unwrap();
    loop {
        if state.shutdown {
            return;
        }
        let Some(armed) = &state.armed else {
            state = shared.wakeup.wait(state).unwrap();
            continue;
        };
        let now = Instant::now();
        if now < armed.deadline {
            let timeout = armed.deadline - now;
            state = shared.wakeup.wait_timeout(state, timeout).unwrap().0;
            continue;
        }
        // Expired: interrupt the test thread at its next RunLoop turn, once.
        let armed = state.armed.take().expect("armed");
        let current = shared.current.clone();
        let generation = armed.generation;
        let payload = TestTimedOut { limit: armed.limit };
        armed.run_loop.invoke(move || {
            if current.load(Ordering::SeqCst) == generation {
                // resume_unwind: no panic-hook message; the runner reports it.
                std::panic::resume_unwind(Box::new(payload));
            }
        });
    }
}
