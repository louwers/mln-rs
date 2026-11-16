// Port-only: a host without threads (cooperative_scheduler.rs). Installing a
// cooperative scheduler is process-wide, so these tests have a binary of
// their own, apart from the gtests that start threads.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use mln_actor::{CooperativeScheduler, Thread, scheduler};
use parking_lot::Mutex;

/// The scheduler, installed by whichever test runs first.
fn cooperative() -> &'static Arc<CooperativeScheduler> {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| scheduler::install_cooperative(Arc::new(CooperativeScheduler::new())));
    scheduler::cooperative().unwrap()
}

#[test]
#[should_panic(expected = "already installed")]
fn installs_once() {
    cooperative();
    scheduler::install_cooperative(Arc::new(CooperativeScheduler::new()));
}

#[test]
fn installing_once_keeps_the_installed_scheduler() {
    let installed = cooperative();
    assert!(Arc::ptr_eq(scheduler::install_cooperative_once(), installed));
}

struct Counter {
    count: Cell<usize>,
    total: Arc<AtomicUsize>,
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.total.store(self.count.get(), Ordering::SeqCst);
    }
}

#[test]
fn a_thread_keeps_its_object_on_a_loop_that_the_host_runs() {
    let cooperative = cooperative();
    let total = Arc::new(AtomicUsize::new(0));
    let t = Arc::clone(&total);
    let thread = Thread::new("Test", move |_, _| Counter { count: Cell::new(0), total: t });
    let main = std::thread::current().id();
    let seen = Arc::new(Mutex::new(Vec::new()));
    for i in 0..10 {
        let seen = Arc::clone(&seen);
        thread.actor().invoke(move |counter: &Counter| {
            assert_eq!(std::thread::current().id(), main);
            counter.count.set(counter.count.get() + 1);
            seen.lock().push(i);
        });
    }
    assert!(seen.lock().is_empty());
    // As the host's pump does: the first pass's messages leave the loop
    // pending until the second.
    cooperative.run_loops();
    assert!(!cooperative.run_loops());
    assert_eq!(*seen.lock(), (0..10).collect::<Vec<_>>());
    drop(thread);
    assert_eq!(total.load(Ordering::SeqCst), 10);
}
