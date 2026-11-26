use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::time::Duration;

use mln_util::pool::Pool;
use parking_lot::Mutex;

use crate::render::orchestrator::RenderOrchestrator;

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

fn orchestrator(pool: &Pool) -> RenderOrchestrator {
    RenderOrchestrator::new(
        true,
        mln_text::local_fonts::LocalGlyphs::default(),
        Arc::default(),
        crate::text::font_face_engine::default_engine(),
        crate::Registry::with_builtins(),
        Arc::new(|| {}),
        pool,
    )
}

fn recorder() -> (Recorder, Arc<Mutex<Option<ThreadId>>>, Arc<AtomicBool>) {
    let dropped_on = Arc::new(Mutex::new(None));
    let done = Arc::new(AtomicBool::new(false));
    (Recorder { dropped_on: Arc::clone(&dropped_on), done: Arc::clone(&done) }, dropped_on, done)
}

#[test]
fn drops_on_a_worker() {
    let orchestrator = orchestrator(&crate::tests::pool());
    let (recorder, dropped_on, done) = recorder();
    orchestrator.drop_on_workers(recorder);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "the value didn't drop");
        std::thread::yield_now();
    }
    let thread = dropped_on.lock().expect("the value dropped");
    assert_ne!(thread, std::thread::current().id());
}

/// The drop waits for a value that a worker drops, and drops one that no
/// worker took yet itself.
#[test]
fn the_orchestrator_waits_for_what_it_drops_on_workers() {
    let orchestrator = orchestrator(&crate::tests::pool());
    let values: Vec<_> = (0..8).map(|_| recorder()).collect();
    let mut done = Vec::new();
    for (recorder, _, value_done) in values {
        orchestrator.drop_on_workers(recorder);
        done.push(value_done);
    }
    drop(orchestrator);
    assert!(done.iter().all(|done| done.load(Ordering::SeqCst)), "the orchestrator's drop waits for the values'");
}
