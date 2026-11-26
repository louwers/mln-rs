//! What a wasm instance needs to run mln-rs in a browser, on the model of
//! examples/web: wasm32-unknown-unknown without atomics, one instance per
//! Web Worker, each with its own memory and one thread.
//!
//! - `pump`: the instance's work on its own event loop. Its queues and
//!   pools wake it, and it runs them in slices of a few milliseconds, each a
//!   task of its own, so that input, worker replies and animation frames
//!   get in between.
//! - `tile_workers`: the map side of tile workers in other Web Workers,
//!   for `mln_engine`'s `RemoteWorkers`, over a `MessagePort` to each.
//! - `tile_worker`: the side of a Web Worker that runs tile workers, one
//!   `TileWorkerHost` per port that a map connected. The app's module
//!   exports its entry point, so that the Web Worker lays out with the
//!   app's registrations.
//! - `shaper`: the font-face shaper as a second module, which the map's
//!   instance loads the first time a style declares font faces.
//!
//! The crate is empty on other targets.

#![cfg(target_arch = "wasm32")]

pub mod pump;
pub mod shaper;
pub mod tile_worker;
pub mod tile_workers;

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    pub(crate) const GLYPH: &str = "mln_web_host::glyph";
}

use wasm_bindgen::JsValue;

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
