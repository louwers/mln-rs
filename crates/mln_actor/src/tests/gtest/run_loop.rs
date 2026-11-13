use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::{RunLoop, Timer};

// gtest: RunLoop.Stop
#[test]
fn stop() {
    let run_loop = RunLoop::new(); // RunLoop::Type::New
    let timer = Timer::new();

    let l = run_loop.clone();
    timer.start(Duration::ZERO, Duration::ZERO, move || l.stop());

    run_loop.run();
}

// gtest: RunLoop.MultipleStop
#[test]
fn multiple_stop() {
    let run_loop = RunLoop::new();
    let timer = Timer::new();

    let l = run_loop.clone();
    timer.start(Duration::ZERO, Duration::ZERO, move || {
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
    timer.start(Duration::ZERO, Duration::ZERO, move || l.stop());
    run_loop.run();

    let second_timeout = Rc::new(Cell::new(false));
    let l = run_loop.clone();
    let s = second_timeout.clone();
    timer.start(Duration::ZERO, Duration::ZERO, move || {
        s.set(true);
        l.stop();
    });
    run_loop.run();

    assert!(second_timeout.get());
}
