// Port-only: a host without threads (cooperative_scheduler.rs). Installing a
// cooperative scheduler is process-wide, so these tests have a binary of
// their own, apart from the gtests that start threads.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use mln_actor::actor::Actor;
use mln_actor::{CooperativeScheduler, RunLoop, Thread, scheduler};
use parking_lot::Mutex;

/// The scheduler, installed by whichever test runs first.
fn cooperative() -> &'static Arc<CooperativeScheduler> {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| scheduler::install_cooperative(Arc::new(CooperativeScheduler::new())));
    scheduler::cooperative().unwrap()
}

#[test]
fn background_and_sequenced_are_the_cooperative_scheduler() {
    let cooperative: Arc<dyn scheduler::Scheduler + Send + Sync> = cooperative().clone();
    assert!(Arc::ptr_eq(&scheduler::get_background(), &cooperative));
    assert!(Arc::ptr_eq(&scheduler::get_sequenced(), &cooperative));
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
fn a_thread_keeps_its_object_on_the_creating_threads_loop() {
    cooperative();
    let run_loop = RunLoop::get();
    let total = Arc::new(AtomicUsize::new(0));
    let t = total.clone();
    let thread = Thread::new("Test", move |_| Counter { count: Cell::new(0), total: t });
    let main = std::thread::current().id();
    let seen = Arc::new(Mutex::new(Vec::new()));
    for i in 0..10 {
        let seen = seen.clone();
        thread.actor().invoke(move |counter: &Counter| {
            assert_eq!(std::thread::current().id(), main);
            counter.count.set(counter.count.get() + 1);
            seen.lock().push(i);
        });
    }
    assert!(seen.lock().is_empty());
    run_loop.wait_for_empty();
    assert_eq!(*seen.lock(), (0..10).collect::<Vec<_>>());
    drop(thread);
    assert_eq!(total.load(Ordering::SeqCst), 10);
}

#[test]
fn a_background_actor_runs_when_the_host_runs_the_tasks() {
    let cooperative = cooperative();
    let (tx, rx) = mpsc::channel();
    let actor = Actor::new(scheduler::get_background(), move |_| tx);
    let main = std::thread::current().id();
    for i in 0..3 {
        actor.self_ref().invoke(move |tx: &mut mpsc::Sender<usize>| {
            assert_eq!(std::thread::current().id(), main);
            tx.send(i).unwrap();
        });
    }
    assert!(rx.try_recv().is_err());
    assert!(!cooperative.run_until(None));
    assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
}
