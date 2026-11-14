use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::time::Duration;

use mln_actor::RunLoop;
use parking_lot::Mutex;

use crate::render_orchestrator::RenderOrchestrator;

/// Records the thread it drops on, after a delay that a drop on the caller's
/// thread or an orchestrator that didn't wait would show.
struct Recorder {
    dropped_on: Arc<Mutex<Option<ThreadId>>>,
    done: Arc<AtomicBool>,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(20));
        *self.dropped_on.lock() = Some(std::thread::current().id());
        self.done.store(true, Ordering::SeqCst);
    }
}

#[test]
fn drops_on_a_worker_and_the_orchestrator_waits_for_it() {
    let _run_loop = RunLoop::new();
    let orchestrator = RenderOrchestrator::new(true, None, Arc::default());
    let dropped_on = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    orchestrator.drop_on_workers(Recorder { dropped_on: Arc::clone(&dropped_on), done: Arc::clone(&done) });
    drop(orchestrator);
    assert!(done.load(Ordering::SeqCst), "the orchestrator's drop waits for the value's");
    let thread = dropped_on.lock().expect("the value dropped");
    assert_ne!(thread, std::thread::current().id());
}
