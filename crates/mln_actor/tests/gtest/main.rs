// Ported C++ gtests of the actor layer: test/actor/*.test.cpp and the
// thread/loop parts of test/util (thread, run_loop, async_task, timer,
// thread_local). Every test carries a `// gtest: Group.Name` marker
// (tools/gtest-parity).
//
// C++ -> Rust in these tests:
// - `Actor<O>` on `Scheduler::GetBackground()` is the pool `mln_actor::actor::
//   Actor<O>` (objects are `Send`, messages are closures over `&mut O`);
//   `util::Thread<O>` and `AspiringActor`/`EstablishedActor` are the
//   thread-pinned actors (messages are closures over `&O`).
// - `std::promise`/`std::future` are `mpsc` channels; a `future.get()` on the
//   test thread is `get()` below, which fails the test after a generous
//   timeout instead of hanging when a message never arrives.
// - gtest `EXPECT_*` inside a message on another thread records the failure;
//   here such checks set a shared flag that the test thread asserts on (a
//   panic on another thread would only kill that thread).

mod actor;
mod actor_ref;
mod async_task;
mod run_loop;
mod settings;
mod thread;
mod thread_local;
mod timer;

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// gtest `EXPECT_*` from other threads: failures are recorded and the test
/// thread checks them with `assert_none`.
#[derive(Clone, Default)]
pub struct Failures(Arc<Mutex<Vec<String>>>);

impl Failures {
    #[track_caller]
    pub fn expect(&self, condition: bool, what: &str) {
        if !condition {
            let location = std::panic::Location::caller();
            self.0.lock().unwrap().push(format!("{location}: {what}"));
        }
    }

    #[track_caller]
    pub fn assert_none(&self) {
        let failures = self.0.lock().unwrap();
        assert!(failures.is_empty(), "failures on other threads:\n{}", failures.join("\n"));
    }
}

/// `future.get()`.
#[track_caller]
#[must_use]
pub fn get<T>(future: &mpsc::Receiver<T>) -> T {
    future.recv_timeout(Duration::from_secs(60)).expect("future not ready after 60 s")
}

/// `future.get()` on a `std::future<void>`.
#[track_caller]
pub fn wait(future: &mpsc::Receiver<()>) {
    future.recv_timeout(Duration::from_secs(60)).expect("future not ready after 60 s");
}

/// `future.wait_for(timeout) == std::future_status::ready`, then `get()`.
#[must_use]
pub fn wait_for<T>(future: &mpsc::Receiver<T>, timeout: Duration) -> Option<T> {
    future.recv_timeout(timeout).ok()
}
