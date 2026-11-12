// Ports test/util/run_loop.test.cpp.

use std::cell::Cell;
use std::rc::Rc;

use mln_actor::{RunLoop, Timer};
use mln_util::chrono::Duration;

// gtest: RunLoop.Stop
#[test]
fn stop() {
    let run_loop = RunLoop::new(); // RunLoop::Type::New
    let timer = Timer::new();

    let l = run_loop.clone();
    timer.start(Duration::zero(), Duration::zero(), move || l.stop());

    run_loop.run();
}

// gtest: RunLoop.MultipleStop
#[test]
fn multiple_stop() {
    let run_loop = RunLoop::new();
    let timer = Timer::new();

    let l = run_loop.clone();
    timer.start(Duration::zero(), Duration::zero(), move || {
        l.stop();
        l.stop();
        l.stop();
        l.stop();
    });

    run_loop.run();
}

// gtest: RunLoop.MultipleRun
#[test]
fn multiple_run() {
    let run_loop = RunLoop::new();
    let timer = Timer::new();

    let l = run_loop.clone();
    timer.start(Duration::zero(), Duration::zero(), move || l.stop());
    run_loop.run();

    let second_timeout = Rc::new(Cell::new(false));
    let l = run_loop.clone();
    let s = second_timeout.clone();
    timer.start(Duration::zero(), Duration::zero(), move || {
        s.set(true);
        l.stop();
    });
    run_loop.run();

    assert!(second_timeout.get());
}

// gtest: RunLoop.PlatformIntegration
#[test]
#[ignore = "TODO(port): RunLoop::setPlatformCallback"]
fn platform_integration() {
    // test/util/run_loop.test.cpp:68: two threads invoke 100000 tasks each on
    // the main loop; the platform callback (on the invoking threads) wakes
    // the main thread, which calls runOnce until all 200000 ran; both counts
    // must be 200000.
}
