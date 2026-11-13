// Ports test/util/timer.test.cpp.
//
// All of them are `TEST_REQUIRES_ACCURATE_TIMING(...)`, i.e. `DISABLED_` on
// the C++ CI; the timing ones are #[ignore]d here for the same reason (the
// test machine is shared). Run them with `--test_arg=--ignored`.
// `MonotonicTimer::now()` and `mln::Clock::now()` (steady_clock) are
// `std::time::Instant`; durations are compared in whole milliseconds, as the
// C++ `duration_cast<Milliseconds>` does.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mln_actor::{RunLoop, Timer};

/// `duration_cast<Milliseconds>(now - first).count()`.
fn elapsed_ms(first: Instant) -> f64 {
    first.elapsed().as_millis() as f64
}

// gtest: Timer.Basic
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn basic() {
    let run_loop = RunLoop::new();

    let interval = 300.0;
    let expected_total_time = interval;

    let first = Instant::now();

    let callback_time: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let (l, c) = (run_loop.clone(), callback_time.clone());
    let callback = move || {
        c.set(Some(elapsed_ms(first)));
        l.stop();
    };

    let timer = Timer::new();
    timer.start(Duration::from_millis(300), Duration::ZERO, callback);

    run_loop.run();

    let total_time = elapsed_ms(first);

    let trace = format!("Timer callback: {:?}", callback_time.get());
    assert!(total_time >= expected_total_time * 0.99, "{trace}, total {total_time}");
    assert!(total_time <= expected_total_time * 1.10, "{trace}, total {total_time}");
}

// gtest: Timer.Repeat
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn repeat() {
    let run_loop = RunLoop::new();
    let timer = Timer::new();

    let count = Rc::new(Cell::new(10u32));
    let (l, c) = (run_loop.clone(), count.clone());
    let callback = move || {
        c.set(c.get() - 1);
        if c.get() == 0 {
            l.stop();
        }
    };

    let interval = 50.0;
    let expected_total_time = interval * f64::from(count.get());

    let first = Instant::now();
    timer.start(Duration::from_millis(50), Duration::from_millis(50), callback);

    run_loop.run();

    let total_time = elapsed_ms(first);

    assert!(total_time >= expected_total_time * 0.8, "{total_time}");
    assert!(total_time <= expected_total_time * 1.3, "{total_time}");
}

// gtest: Timer.Stop
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn stop() {
    let run_loop = RunLoop::new();

    let timer1 = Rc::new(Timer::new());
    let timer2 = Timer::new();

    let interval1 = Duration::from_millis(50);
    let interval2 = Duration::from_millis(250);
    let expected_total_time = 250.0;

    let count = Rc::new(Cell::new(0));

    let (c, t1) = (count.clone(), Rc::downgrade(&timer1));
    let callback1 = move || {
        c.set(c.get() + 1);
        if let Some(t1) = t1.upgrade() {
            t1.stop();
        }
    };

    let (c, l) = (count.clone(), run_loop.clone());
    let callback2 = move || {
        c.set(c.get() + 1);
        l.stop();
    };

    let first = Instant::now();
    timer1.start(interval1, interval1, callback1);
    timer2.start(interval2, Duration::ZERO, callback2);

    run_loop.run();

    let total_time = elapsed_ms(first);

    assert_eq!(count.get(), 2);

    assert!(total_time >= expected_total_time * 0.8, "{total_time}");
    assert!(total_time <= expected_total_time * 1.2, "{total_time}");
}

// gtest: Timer.DestroyShouldStop
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn destroy_should_stop() {
    let run_loop = RunLoop::new();

    let timer1: Rc<RefCell<Option<Timer>>> = Rc::new(RefCell::new(Some(Timer::new())));
    let timer2 = Timer::new();

    let interval1 = Duration::from_millis(50);
    let interval2 = Duration::from_millis(250);
    let expected_total_time = 250.0;

    let count = Rc::new(Cell::new(0));

    let (c, t1) = (count.clone(), timer1.clone());
    let callback1 = move || {
        c.set(c.get() + 1);
        let timer = t1.borrow_mut().take();
        drop(timer);
    };

    let (c, l) = (count.clone(), run_loop.clone());
    let callback2 = move || {
        c.set(c.get() + 1);
        l.stop();
    };

    let first = Instant::now();
    timer1.borrow().as_ref().unwrap().start(interval1, interval1, callback1);
    timer2.start(interval2, Duration::ZERO, callback2);

    run_loop.run();

    let total_time = elapsed_ms(first);

    assert_eq!(count.get(), 2);

    assert!(total_time >= expected_total_time * 0.8, "{total_time}");
    assert!(total_time <= expected_total_time * 1.2, "{total_time}");
}

// gtest: Timer.StoppedDuringExpiration
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn stopped_during_expiration() {
    // The idea is to have original timer cancellation and expiration roughly at
    // the same time. In this case some timer backens (e.g.
    // asio::high_resolution_timer) may call the expiration callback with good
    // status while the timer may not expect it.

    let run_loop = RunLoop::new();

    let timer = Rc::new(Timer::new());
    let loop_stop_timer = Timer::new();
    let expire_timeout = 50.0;

    let timer_callback = || {
        // we cannot expect much here as in some cases timer may be finished earlier
        // than the loop stop timer (and thus the callback will be called)
    };

    let (t, l) = (timer.clone(), run_loop.clone());
    let loop_stop_timer_callback = move || {
        t.stop();
        l.stop();
    };

    let first = Instant::now();
    loop_stop_timer.start(Duration::from_millis(50), Duration::from_millis(0), loop_stop_timer_callback);
    timer.start(Duration::from_millis(50), Duration::from_millis(0), timer_callback);

    run_loop.run();

    let total_time = elapsed_ms(first);

    assert!(total_time >= expire_timeout * 0.8, "{total_time}");
    assert!(total_time <= expire_timeout * 1.3, "{total_time}");
}

// gtest: Timer.StoppedAfterExpiration
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn stopped_after_expiration() {
    let run_loop = RunLoop::new();

    let timer = Timer::new();
    let _loop_stop_timer = Timer::new();
    let expire_timeout = 50.0;

    let callback_fired = Rc::new(Cell::new(false));

    let c = callback_fired.clone();
    let timer_callback = move || c.set(true);

    let first = Instant::now();
    timer.start(Duration::from_millis(50), Duration::from_millis(0), timer_callback);

    // poll until the timer expires
    let expire_wait_interval = expire_timeout * 2.0;
    let start_wait_time = Instant::now();
    let mut wait_duration = 0.0;
    while wait_duration < expire_wait_interval {
        wait_duration = elapsed_ms(start_wait_time);
    }
    timer.stop();
    run_loop.run_once();

    let total_time = elapsed_ms(first);

    assert!(!callback_fired.get());
    assert!(total_time >= expire_wait_interval * 0.8, "{total_time}");
    assert!(total_time <= expire_wait_interval * 1.2, "{total_time}");
}

// gtest: Timer.StartOverrides
#[test]
#[ignore = "TEST_REQUIRES_ACCURATE_TIMING (DISABLED_ on the C++ CI)"]
fn start_overrides() {
    let run_loop = RunLoop::new();

    let timer = Rc::new(Timer::new());

    let interval1 = 50.0;
    let interval2 = 250.0;
    let expected_total_time = interval1 + interval2;

    let count = Rc::new(Cell::new(0));

    let (c, l) = (count.clone(), run_loop.clone());
    let callback2 = move || {
        c.set(c.get() + 1);
        l.stop();
    };

    let (c, t) = (count.clone(), Rc::downgrade(&timer));
    let callback2 = RefCell::new(Some(callback2));
    let callback1 = move || {
        c.set(c.get() + 1);
        if let (Some(t), Some(callback2)) = (t.upgrade(), callback2.borrow_mut().take()) {
            t.start(Duration::from_millis(250), Duration::ZERO, callback2);
        }
    };

    let first = Instant::now();
    timer.start(Duration::from_millis(50), Duration::ZERO, callback1);

    run_loop.run();

    let total_time = elapsed_ms(first);

    assert_eq!(count.get(), 2);

    assert!(total_time >= expected_total_time * 0.8, "{total_time}");
    assert!(total_time <= expected_total_time * 1.2, "{total_time}");
}

// gtest: Timer.CanStopNonStartedTimer
#[test]
fn can_stop_non_started_timer() {
    let _run_loop = RunLoop::new();
    let timer = Timer::new();
    timer.stop();
}
