//! What a wasm instance needs to run mln-rs in a browser, on the model of
//! examples/web: wasm32-unknown-unknown without atomics, one instance per
//! Web Worker, each with its own memory and one thread.
//!
//! - `pump`: the instance's work on its own event loop. Its `RunLoop`s and
//!   the cooperative scheduler wake it, and it runs them in slices of a few
//!   milliseconds, each a task of its own, so that input, worker replies
//!   and animation frames get in between.
//! - `tile_workers`: the map side of tile workers in other Web Workers,
//!   for `mln_renderer`'s `SerializingTransport`, over a `MessagePort` to each.
//! - `tile_worker`: the side of a Web Worker that runs tile workers, one
//!   `TileWorkerHost` per port that a map connected.
//! - `shaper`: the font-face shaper as a second module, which the map's
//!   instance loads the first time a style declares font faces.
//!
//! The crate is empty on other targets.

#![cfg(target_arch = "wasm32")]

pub mod pump;
pub mod shaper;
pub mod tile_worker;
pub mod tile_workers;

use std::sync::Arc;

use mln_actor::{CooperativeScheduler, scheduler};
use wasm_bindgen::JsValue;

/// The instance's cooperative scheduler, installed on the first call. An
/// instance has no threads, so it must come before any map, file source or
/// `Thread` exists, which would start threads otherwise.
#[must_use]
pub fn install_cooperative_scheduler() -> Arc<CooperativeScheduler> {
    scheduler::install_cooperative_once().clone()
}

/// Logs a panic's message to the console, and leaves it in the global
/// `mlnPanic`, for the JavaScript that runs the instance: wasm32-unknown-unknown
/// aborts on a panic, which traps the instance, and nothing may call into it
/// after that.
#[wasm_bindgen::prelude::wasm_bindgen(js_name = mlnWebSetPanicHook)]
pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = JsValue::from_str(&info.to_string());
        web_sys::console::error_1(&message);
        let _ = js_sys::Reflect::set(&js_sys::global(), &JsValue::from_str("mlnPanic"), &message);
    }));
}
