//! The instance's work, run on the browser's event loop of its thread.
//!
//! An instance has one thread, which the browser's event loop runs, so
//! nothing of the instance may block it. The instance's `RunLoop`s and its
//! cooperative scheduler call [`waker`] when they get work, which posts one
//! message to a `MessageChannel` of the pump's own. Its task runs a slice:
//! each [`Step`], then the scheduler's tasks until the slice's time is
//! spent, then each step again for what those tasks delivered. Work that
//! remains, or that arrives meanwhile, posts the next slice. A message
//! rather than `setTimeout`, whose delay grows once timeouts nest, so that
//! the next slice comes as soon as the event loop has run what waits ahead
//! of it: input, worker replies, animation frames.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mln_actor::scheduler;
use mln_util::time::Instant;
use wasm_bindgen::prelude::*;

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
    slice: Cell<Duration>,
    steps: RefCell<Vec<Rc<Entry>>>,
    _on_message: Closure<dyn FnMut()>,
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
            let pump = Weak::clone(pump);
            let on_message = Closure::<dyn FnMut()>::new(move || {
                if let Some(pump) = pump.upgrade() {
                    pump.run_slice();
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
                slice: Cell::new(Duration::from_millis(4)),
                steps: RefCell::new(Vec::new()),
                _on_message: on_message,
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

    fn run_slice(&self) {
        self.scheduled.set(false);
        if self.stopped.get() || self.running.replace(true) {
            return;
        }
        let deadline = Instant::now() + self.slice.get();
        self.run_steps();
        let more = scheduler::cooperative().is_some_and(|scheduler| scheduler.run_until(Some(deadline)));
        self.run_steps();
        self.running.set(false);
        if more {
            self.wake();
        }
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

/// Posts a slice, unless one is on its way.
pub fn wake() {
    PUMP.with(|pump| pump.wake());
}

/// [`wake`], for `RunLoop::set_waker` and `CooperativeScheduler::set_waker`.
/// The instance has one thread, so it runs on the pump's.
#[must_use]
pub fn waker() -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(wake)
}

/// Sets how long each slice runs the scheduler's tasks: 4 ms unless set.
/// The steps run in every slice, however long they take.
pub(crate) fn set_slice(slice: Duration) {
    PUMP.with(|pump| pump.slice.set(slice));
}

/// Stops running slices, for good: for the JavaScript that runs an instance
/// and gives up on it, such as once another instance it works with panicked.
#[wasm_bindgen(js_name = mlnWebStopPump)]
pub fn stop() {
    PUMP.with(|pump| pump.stopped.set(true));
}
