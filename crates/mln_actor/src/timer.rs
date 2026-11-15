// A timer belongs to the RunLoop of the thread that created it and fires in
// that loop's `run_once` (timers run first, as in `uv_run`). Like the C++
// timer, the durations are truncated to milliseconds, and a timer does not
// keep `RunLoop::run()` alive on its own.
//
// The loop keeps its running timers in a heap ordered by due time. Starting,
// stopping or rearming a timer leaves its old entry behind, which the loop
// skips when it reaches it, and drops when stale entries pile up.

use std::cell::{Cell, RefCell};
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::rc::{Rc, Weak};
use std::time::Duration;

use mln_util::time::Instant;

use crate::run_loop::RunLoop;

/// The running timers of a loop.
#[derive(Default)]
pub(crate) struct Timers {
    heap: RefCell<BinaryHeap<Reverse<Entry>>>,
    /// libuv orders timers with the same due time by start order.
    next_start_id: Cell<u64>,
    /// The heap's size after it last dropped its stale entries.
    compacted_len: Cell<usize>,
}

/// A timer's place in the heap, which holds while the timer's `armed` is
/// still the entry's.
struct Entry {
    due: Instant,
    start_id: u64,
    armed: u64,
    timer: Weak<TimerHandle>,
}

impl Entry {
    fn key(&self) -> (Instant, u64, u64) {
        (self.due, self.start_id, self.armed)
    }

    /// The timer, if this entry is still its due time.
    fn live(&self) -> Option<Rc<TimerHandle>> {
        self.timer.upgrade().filter(|timer| timer.armed.get() == self.armed)
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl Timers {
    fn next_start_id(&self) -> u64 {
        let id = self.next_start_id.get().wrapping_add(1);
        self.next_start_id.set(id);
        id
    }

    /// Arms `timer` for `due`, or disarms it for `None`; its entry before
    /// goes stale.
    fn arm(&self, timer: &Rc<TimerHandle>, due: Option<Instant>) {
        let armed = timer.armed.get().wrapping_add(1);
        timer.armed.set(armed);
        let Some(due) = due else { return };
        let mut heap = self.heap.borrow_mut();
        heap.push(Reverse(Entry { due, start_id: timer.start_id.get(), armed, timer: Rc::downgrade(timer) }));
        // Timers that restart before they fire leave an entry each.
        if heap.len() > 64 && heap.len() > 2 * self.compacted_len.get() {
            heap.retain(|Reverse(entry)| entry.live().is_some());
            self.compacted_len.set(heap.len());
        }
    }

    /// Takes the timers due at `now`, earliest first (ties in start order).
    pub(crate) fn take_due(&self, now: Instant) -> Vec<(Rc<TimerHandle>, u64)> {
        let mut heap = self.heap.borrow_mut();
        let mut due = Vec::new();
        while let Some(Reverse(entry)) = heap.peek() {
            if entry.due > now {
                break;
            }
            let Some(Reverse(entry)) = heap.pop() else { break };
            if let Some(timer) = entry.live() {
                due.push((timer, entry.armed));
            }
        }
        due
    }

    /// The earliest due time of the running timers.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        let mut heap = self.heap.borrow_mut();
        while let Some(Reverse(entry)) = heap.peek() {
            if entry.live().is_some() {
                return Some(entry.due);
            }
            heap.pop();
        }
        None
    }
}

/// The state of a `uv_timer_t`.
pub(crate) struct TimerHandle {
    timers: Weak<Timers>,
    /// `uv_timer_start`'s repeat in milliseconds (0: no repeat).
    repeat_ms: Cell<u64>,
    start_id: Cell<u64>,
    /// Bumped by every arming, so that the heap's older entries go stale.
    armed: Cell<u64>,
    /// Bumped by every start/stop, so a callback that restarts its timer
    /// keeps the new callback.
    generation: Cell<u64>,
    cb: RefCell<Option<Box<dyn FnMut()>>>,
}

/// Whole milliseconds, as libuv counts them.
fn to_uv_millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn add_millis(now: Instant, ms: u64) -> Option<Instant> {
    now.checked_add(Duration::from_millis(ms))
}

impl TimerHandle {
    /// Called by the `RunLoop` when the timer is due, unless an earlier
    /// callback of the same pass rearmed or stopped it since `armed`:
    /// reschedules a repeating timer (`uv_timer_again`, before the callback
    /// as libuv does), then runs the callback.
    pub(crate) fn fire(self: &Rc<Self>, armed: u64, now: Instant) {
        if self.armed.get() != armed {
            return;
        }
        let repeat = self.repeat_ms.get();
        let due = if repeat > 0 { add_millis(now, repeat) } else { None };
        if let Some(timers) = self.timers.upgrade() {
            timers.arm(self, due);
        }
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

pub struct Timer {
    handle: Rc<TimerHandle>,
}

impl Timer {
    /// Creates a timer on the current thread's `RunLoop`.
    #[must_use]
    pub fn new() -> Self {
        Self::on(&RunLoop::get())
    }

    /// Creates a timer that fires in `run_loop`, which runs on this thread.
    #[must_use]
    pub fn on(run_loop: &RunLoop) -> Self {
        let handle = Rc::new(TimerHandle {
            timers: Rc::downgrade(run_loop.timers()),
            repeat_ms: Cell::new(0),
            start_id: Cell::new(0),
            armed: Cell::new(0),
            generation: Cell::new(0),
            cb: RefCell::new(None),
        });
        Timer { handle }
    }

    pub fn start(&self, timeout: Duration, repeat: Duration, cb: impl FnMut() + 'static) {
        let handle = &self.handle;
        *handle.cb.borrow_mut() = Some(Box::new(cb));
        handle.generation.set(handle.generation.get().wrapping_add(1));
        handle.repeat_ms.set(to_uv_millis(repeat));
        let Some(timers) = handle.timers.upgrade() else { return };
        handle.start_id.set(timers.next_start_id());
        // uv_update_time, then uv_timer_start
        timers.arm(handle, add_millis(Instant::now(), to_uv_millis(timeout)));
    }

    pub fn stop(&self) {
        let handle = &self.handle;
        *handle.cb.borrow_mut() = None;
        handle.generation.set(handle.generation.get().wrapping_add(1));
        handle.armed.set(handle.armed.get().wrapping_add(1));
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
        let c = Rc::clone(&count);
        let handle = run_loop.handle();
        timer.start(Duration::from_millis(20), Duration::ZERO, move || {
            c.set(c.get() + 1);
            handle.stop();
        });
        let started = Instant::now();
        run_loop.run();
        assert_eq!(count.get(), 1);
        assert!(started.elapsed() >= Duration::from_millis(20));
        run_loop.run_once();
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn repeats_until_stopped() {
        let run_loop = RunLoop::new();
        let count = Rc::new(Cell::new(0));
        let timer = Rc::new(Timer::new());
        let (c, t) = (Rc::clone(&count), Rc::downgrade(&timer));
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
    fn dropped_timer_does_not_fire() {
        let run_loop = RunLoop::new();
        let fired = Rc::new(Cell::new(false));
        let timer = Timer::new();
        let f = Rc::clone(&fired);
        timer.start(Duration::ZERO, Duration::ZERO, move || f.set(true));
        drop(timer);
        run_loop.run_once();
        assert!(!fired.get());
    }

    #[test]
    fn a_timer_restarted_before_it_fires_leaves_few_stale_entries() {
        let run_loop = RunLoop::new();
        let timer = Timer::on(&run_loop);
        for _ in 0..10_000 {
            timer.start(Duration::from_secs(3600), Duration::ZERO, || {});
        }
        assert!(run_loop.timers().heap.borrow().len() <= 128);
        assert!(run_loop.timers().next_due().is_some());
        timer.stop();
        assert_eq!(run_loop.timers().next_due(), None);
    }

    #[test]
    fn zero_timeout_fires_on_next_run_once() {
        let run_loop = RunLoop::new();
        let fired = Rc::new(Cell::new(0));
        let timer = Timer::new();
        let f = Rc::clone(&fired);
        timer.start(Duration::ZERO, Duration::ZERO, move || f.set(f.get() + 1));
        run_loop.run_once();
        assert_eq!(fired.get(), 1);
        run_loop.run_once();
        assert_eq!(fired.get(), 1);
    }
}
