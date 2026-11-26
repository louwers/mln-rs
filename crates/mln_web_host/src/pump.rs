//! The instance's work, run on the browser's event loop of its thread.
//!
//! An instance has one thread, which the browser's event loop runs, so
//! nothing of the instance may block it. The instance's queues and pools
//! call [`waker`] when they get work, which posts one message to a
//! `MessageChannel` of the pump's own. Its task runs a slice: each [`Step`],
//! then the pools' tasks until the slice's time is spent, then each step
//! again for what those tasks delivered. Work that
//! remains, or that arrives meanwhile, posts the next slice. A message
//! rather than `setTimeout`, whose delay grows once timeouts nest, so that
//! the next slice comes as soon as the event loop has run what waits ahead
//! of it: input, worker replies, animation frames. Work that waits for a
//! time, such as a request's retry, asks for a slice then with [`wake_at`],
//! which arms one timeout, for the earliest such time.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mln_style::time::Instant;
use mln_util::pool::Pool;
use wasm_bindgen::prelude::*;

/// How long each slice runs the pools' tasks. The steps run in every slice,
/// however long they take.
const SLICE: Duration = Duration::from_millis(4);

/// What a step runs in each slice. It returns `false` once it is done, which
/// removes it.
type StepFn = Box<dyn FnMut() -> bool>;

struct Entry {
    run: RefCell<StepFn>,
    live: Cell<bool>,
}

struct Pump {
    /// `None` where the browser gave none, which leaves the instance's work
    /// to its host's frames.
    channel: Option<web_sys::MessageChannel>,
    /// A slice's message is on its way.
    scheduled: Cell<bool>,
    running: Cell<bool>,
    stopped: Cell<bool>,
    /// The time the slices spent on the pools' tasks since
    /// [`take_pool_time`] last took it.
    pool_time: Cell<Duration>,
    steps: RefCell<Vec<Rc<Entry>>>,
    pools: RefCell<Vec<Rc<Pool>>>,
    /// The timeout armed for the earliest time that [`wake_at`] was given
    /// since the last one fired: that time, and the timeout's id.
    timeout: Cell<Option<(Instant, JsValue)>>,
    _on_message: Closure<dyn FnMut()>,
    on_timeout: Closure<dyn FnMut()>,
}

#[wasm_bindgen]
extern "C" {
    /// The global `setTimeout`, a window's or a worker's.
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(handler: &js_sys::Function, delay: i32) -> JsValue;

    /// The global `clearTimeout`.
    #[wasm_bindgen(js_name = clearTimeout)]
    fn clear_timeout(id: &JsValue);
}

thread_local! {
    static PUMP: Rc<Pump> = Pump::new();
}

impl Pump {
    fn new() -> Rc<Pump> {
        Rc::new_cyclic(|pump: &Weak<Pump>| {
            let channel = web_sys::MessageChannel::new()
                .inspect_err(|error| {
                    web_sys::console::error_2(&JsValue::from_str("the pump has no MessageChannel:"), error);
                })
                .ok();
            let weak = Weak::clone(pump);
            let on_message = Closure::<dyn FnMut()>::new(move || {
                if let Some(pump) = weak.upgrade() {
                    pump.run_slice();
                }
            });
            let weak = Weak::clone(pump);
            let on_timeout = Closure::<dyn FnMut()>::new(move || {
                if let Some(pump) = weak.upgrade() {
                    pump.timeout.take();
                    pump.wake();
                }
            });
            if let Some(channel) = &channel {
                channel.port1().set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            }
            Pump {
                channel,
                scheduled: Cell::new(false),
                running: Cell::new(false),
                stopped: Cell::new(false),
                pool_time: Cell::new(Duration::ZERO),
                steps: RefCell::new(Vec::new()),
                pools: RefCell::new(Vec::new()),
                timeout: Cell::new(None),
                _on_message: on_message,
                on_timeout,
            }
        })
    }

    fn wake(&self) {
        if let Some(channel) = &self.channel
            && !self.stopped.get()
            && !self.scheduled.replace(true)
        {
            let _ = channel.port2().post_message(&JsValue::NULL);
        }
    }

    /// Arms the timeout for `deadline`, unless one is armed for that time
    /// or sooner.
    fn wake_at(&self, deadline: Instant) {
        if self.stopped.get() {
            return;
        }
        match self.timeout.take() {
            Some(armed) if armed.0 <= deadline => {
                self.timeout.set(Some(armed));
                return;
            }
            Some((_, id)) => clear_timeout(&id),
            None => {}
        }
        // Rounded up, so that the slice comes once the time has passed.
        let delay = deadline.saturating_duration_since(Instant::now()).as_micros().div_ceil(1000);
        let delay = i32::try_from(delay).unwrap_or(i32::MAX);
        self.timeout.set(Some((deadline, set_timeout(self.on_timeout.as_ref().unchecked_ref(), delay))));
    }

    fn run_slice(&self) {
        self.scheduled.set(false);
        if self.stopped.get() || self.running.replace(true) {
            return;
        }
        let deadline = Instant::now() + SLICE;
        self.run_steps();
        let tasks = Instant::now();
        let more = self.run_tasks(deadline);
        self.pool_time.set(self.pool_time.get() + tasks.elapsed());
        self.run_steps();
        self.running.set(false);
        if more {
            self.wake();
        }
    }

    /// Runs the pools' tasks until `deadline`. Returns whether work remains.
    fn run_tasks(&self, deadline: Instant) -> bool {
        // Not borrowed while the tasks run, which may add or drop pools.
        let pools: Vec<Rc<Pool>> = self.pools.try_borrow().map(|pools| pools.clone()).unwrap_or_default();
        let mut more = false;
        for pool in pools {
            more |= pool.run_until(Some(deadline));
        }
        more
    }

    fn run_steps(&self) {
        let steps: Vec<Rc<Entry>> = self.steps.borrow().clone();
        for entry in steps {
            // A step that runs already, from a step that runs the pump's
            // work itself, runs once.
            let Ok(mut run) = entry.run.try_borrow_mut() else { continue };
            if entry.live.get() && !run() {
                entry.live.set(false);
            }
        }
        self.steps.borrow_mut().retain(|entry| entry.live.get());
    }
}

/// Runs a step in every slice of the pump, until it returns `false` or the
/// guard drops, unless [`detach`](Self::detach)ed.
#[must_use = "the step stops when its guard drops"]
pub struct Step {
    entry: Option<Rc<Entry>>,
}

impl Step {
    /// Leaves the step running until it returns `false`.
    pub fn detach(mut self) {
        self.entry = None;
    }
}

impl Drop for Step {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            entry.live.set(false);
            // Not inside a slice, which retains the live steps after.
            let _ = PUMP.try_with(|pump| {
                if let Ok(mut steps) = pump.steps.try_borrow_mut() {
                    steps.retain(|other| !Rc::ptr_eq(other, &entry));
                }
            });
        }
    }
}

/// Runs `step` in each slice from the next one on, and posts one. `step`
/// returns whether it should keep running. It runs on this thread and may
/// add or drop steps, but must not run a slice itself.
pub fn add(step: impl FnMut() -> bool + 'static) -> Step {
    let entry = Rc::new(Entry { run: RefCell::new(Box::new(step)), live: Cell::new(true) });
    PUMP.with(|pump| {
        pump.steps.borrow_mut().push(Rc::clone(&entry));
        pump.wake();
    });
    Step { entry: Some(entry) }
}

/// Runs `pool`'s tasks in each slice, until the guard drops. Its waker
/// should be [`waker`].
#[must_use = "the pool stops running when its guard drops"]
pub fn add_pool(pool: Pool) -> PoolGuard {
    let pool = Rc::new(pool);
    PUMP.with(|pump| {
        // Never borrowed across a call out of the pump.
        if let Ok(mut pools) = pump.pools.try_borrow_mut() {
            pools.push(Rc::clone(&pool));
        }
        pump.wake();
    });
    PoolGuard { pool }
}

/// Runs a pool in the pump's slices until it drops.
pub struct PoolGuard {
    pool: Rc<Pool>,
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        let _ = PUMP.try_with(|pump| {
            if let Ok(mut pools) = pump.pools.try_borrow_mut() {
                pools.retain(|other| !Rc::ptr_eq(other, &self.pool));
            }
        });
    }
}

/// Posts a slice, unless one is on its way.
pub fn wake() {
    PUMP.with(|pump| pump.wake());
}

/// Posts a slice once `deadline` has passed, for work that waits for a
/// time, such as a queue's timed task or a request's retry. A step that
/// has such work asks again in each slice for the earliest time it waits
/// for, as long as it waits.
pub fn wake_at(deadline: Instant) {
    PUMP.with(|pump| pump.wake_at(deadline));
}

/// [`wake`], for `Queue::set_waker` and `Pool::set_waker`.
/// The instance has one thread, so it runs on the pump's.
#[must_use]
pub fn waker() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(wake)
}

/// The time the slices spent on the pools' tasks since the last call, for
/// a host that reports the instance's work between its frames.
#[must_use]
pub fn take_pool_time() -> Duration {
    PUMP.with(|pump| pump.pool_time.take())
}

/// Stops running slices, for good: for the JavaScript that runs an instance
/// and gives up on it, such as once another instance it works with panicked.
#[wasm_bindgen(js_name = mlnWebStopPump)]
pub fn stop() {
    PUMP.with(|pump| {
        pump.stopped.set(true);
        if let Some((_, id)) = pump.timeout.take() {
            clear_timeout(&id);
        }
    });
}
