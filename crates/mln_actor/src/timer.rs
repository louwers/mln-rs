// Ports include/mln/util/timer.hpp and platform/default/src/mln/util/timer.cpp
// (the libuv `uv_timer_t`).
//
// A timer belongs to the RunLoop of the thread that created it and fires in
// that loop's `run_once` (timers run first, as in `uv_run`). Like the C++
// timer, the durations are truncated to milliseconds, and a timer does not
// keep `RunLoop::run()` alive on its own.
//
// Quirk ported: C++ casts the millisecond count to `uint64_t`, so a negative
// timeout or repeat becomes a huge value and the timer never fires.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_util::chrono::Duration;
use mln_util::time::Instant;

use crate::run_loop::RunLoop;

/// The state of a `uv_timer_t`, shared with the `RunLoop`.
pub(crate) struct TimerHandle {
    /// When the timer fires next; `None` when stopped or never.
    pub(crate) due: Cell<Option<Instant>>,
    /// `uv_timer_start`'s repeat in milliseconds (0: no repeat).
    repeat_ms: Cell<u64>,
    /// libuv orders timers with the same due time by start order.
    pub(crate) start_id: Cell<u64>,
    /// Bumped by every start/stop, so a callback that restarts its timer
    /// keeps the new callback.
    generation: Cell<u64>,
    cb: RefCell<Option<Box<dyn FnMut()>>>,
}

thread_local! {
    static NEXT_START_ID: Cell<u64> = const { Cell::new(0) };
}

/// `std::chrono::duration_cast<Milliseconds>(d).count()` cast to `uint64_t`.
fn to_uv_millis(d: Duration) -> u64 {
    d.as_millis() as u64
}

fn add_millis(now: Instant, ms: u64) -> Option<Instant> {
    now.checked_add(std::time::Duration::from_millis(ms))
}

impl TimerHandle {
    /// Called by the `RunLoop` when the timer is due: reschedules a repeating
    /// timer (`uv_timer_again`, before the callback as libuv does), then
    /// runs the callback.
    pub(crate) fn fire(&self, now: Instant) {
        let repeat = self.repeat_ms.get();
        self.due.set(if repeat > 0 { add_millis(now, repeat) } else { None });
        let generation = self.generation.get();
        let cb = self.cb.borrow_mut().take();
        if let Some(mut cb) = cb {
            cb();
            // The callback may have stopped or restarted the timer.
            if self.generation.get() == generation {
                *self.cb.borrow_mut() = Some(cb);
            }
        }
    }
}

/// `util::Timer`.
pub struct Timer {
    handle: Rc<TimerHandle>,
}

impl Timer {
    /// `Timer()`: a timer on the current thread's `RunLoop`.
    #[must_use]
    pub fn new() -> Self {
        let handle = Rc::new(TimerHandle {
            due: Cell::new(None),
            repeat_ms: Cell::new(0),
            start_id: Cell::new(0),
            generation: Cell::new(0),
            cb: RefCell::new(None),
        });
        RunLoop::get().register_timer(&handle);
        Timer { handle }
    }

    /// `start(Duration timeout, Duration repeat, std::function<void()>&&)`.
    pub fn start(&self, timeout: Duration, repeat: Duration, cb: impl FnMut() + 'static) {
        let handle = &self.handle;
        *handle.cb.borrow_mut() = Some(Box::new(cb));
        handle.generation.set(handle.generation.get().wrapping_add(1));
        handle.repeat_ms.set(to_uv_millis(repeat));
        // uv_update_time, then uv_timer_start
        handle.due.set(add_millis(Instant::now(), to_uv_millis(timeout)));
        handle.start_id.set(NEXT_START_ID.with(|id| {
            let next = id.get().wrapping_add(1);
            id.set(next);
            next
        }));
    }

    /// `stop()`.
    pub fn stop(&self) {
        let handle = &self.handle;
        *handle.cb.borrow_mut() = None;
        handle.generation.set(handle.generation.get().wrapping_add(1));
        handle.due.set(None);
    }
}

impl Default for Timer {
    fn default() -> Self {
        Timer::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_loop::RunLoop;

    #[test]
    fn fires_once_after_timeout() {
        let run_loop = RunLoop::new();
        let count = Rc::new(Cell::new(0));
        let timer = Timer::new();
        let c = count.clone();
        let handle = run_loop.handle();
        timer.start(Duration::from_millis(20), Duration::zero(), move || {
            c.set(c.get() + 1);
            handle.stop();
        });
        let started = Instant::now();
        run_loop.run();
        assert_eq!(count.get(), 1);
        assert!(started.elapsed() >= std::time::Duration::from_millis(20));
        run_loop.run_once();
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn repeats_until_stopped() {
        // Ports test/util/timer.test.cpp `Timer.Repeat`.
        let run_loop = RunLoop::new();
        let count = Rc::new(Cell::new(0));
        let timer = Rc::new(Timer::new());
        let (c, t) = (count.clone(), Rc::downgrade(&timer));
        let handle = run_loop.handle();
        timer.start(Duration::from_millis(5), Duration::from_millis(5), move || {
            c.set(c.get() + 1);
            if c.get() == 5 {
                if let Some(t) = t.upgrade() {
                    t.stop();
                }
                handle.stop();
            }
        });
        run_loop.run();
        assert_eq!(count.get(), 5);
    }

    #[test]
    fn negative_timeout_never_fires() {
        let run_loop = RunLoop::new();
        let fired = Rc::new(Cell::new(false));
        let timer = Timer::new();
        let f = fired.clone();
        timer.start(Duration::from_millis(-1), Duration::zero(), move || f.set(true));
        run_loop.run_once();
        assert!(!fired.get());
    }

    #[test]
    fn dropped_timer_does_not_fire() {
        let run_loop = RunLoop::new();
        let fired = Rc::new(Cell::new(false));
        let timer = Timer::new();
        let f = fired.clone();
        timer.start(Duration::zero(), Duration::zero(), move || f.set(true));
        drop(timer);
        run_loop.run_once();
        assert!(!fired.get());
    }

    #[test]
    fn zero_timeout_fires_on_next_run_once() {
        let run_loop = RunLoop::new();
        let fired = Rc::new(Cell::new(0));
        let timer = Timer::new();
        let f = fired.clone();
        timer.start(Duration::zero(), Duration::zero(), move || f.set(f.get() + 1));
        run_loop.run_once();
        assert_eq!(fired.get(), 1);
        run_loop.run_once();
        assert_eq!(fired.get(), 1);
    }
}
