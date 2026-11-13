//! The browser build of mln-rs: the libraries built for
//! wasm32-unknown-unknown (no atomics) and driven from JavaScript through
//! wasm-bindgen. The page loads one wasm module, and Web Workers instantiate
//! it, each instance with its own memory:
//!
//! - In the map's Web Worker, `WebMap` (map.rs): the map, its renderer (the
//!   browser's WebGPU through wgpu, or its WebGL2 through `mln_gl` in the
//!   `webgl` build, into the page's canvas as an `OffscreenCanvas`) and its
//!   file sources. Each animation frame runs the `RunLoop`, draws, and runs the
//!   cooperative scheduler's tasks until the frame's budget is spent; between
//!   frames, `mln_web_host::pump` runs what arrives.
//! - In each tile worker's Web Worker, `mln_web_host`'s tile worker side: a
//!   `TileWorkerHost` on the instance's cooperative scheduler. The map's
//!   `SerializingTransport` puts each tile's worker (geometry, raster or
//!   raster-dem) in one of them and talks to it only through bytes, over a
//!   `MessageChannel` between the two Web Workers.
//! - In the map's Web Worker again, once a style declares font faces: the
//!   shaper module (`crates/mln_web_shaper`), which shapes and rasterizes them
//!   (`mln_web_host::shaper`). Only the map's Web Worker loads it, and maps
//!   without font faces never do.
//!
//! The page's thread runs no wasm: it only starts the Web Workers and
//! forwards to the map's.
//!
//! An instance has no threads, so `start` installs a `CooperativeScheduler`
//! before anything else runs, and the instance's tasks run when its Web
//! Worker runs them.
//!
//! web/main.ts, web/map-worker.ts and web/tile-worker.ts are the page's side.

#![cfg(target_arch = "wasm32")]

mod map;

use std::time::Duration;

pub use map::{WebMap, set_preloads};
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_util::time::Instant;
use wasm_bindgen::prelude::*;

/// Runs when an instance starts, in each Web Worker, before any
/// map or file source exists (they would start threads otherwise).
#[wasm_bindgen(start)]
fn start() {
    // The Web Worker's script reads a panic's message from `mlnPanic`.
    mln_web_host::set_panic_hook();
    Log::set_observer(Box::new(ConsoleLog));
    let _ = mln_web_host::install_cooperative_scheduler();
}

/// Panics with `message`. Only check/check.mjs calls it (the page's `panic`
/// parameter), to test that a panic stops the page.
#[wasm_bindgen]
pub fn panic_for_check(message: &str) {
    panic!("{message}");
}

/// The port's log records go to the console.
struct ConsoleLog;

impl Observer for ConsoleLog {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        let line = JsValue::from_str(&format!("[{}] {msg}", event.as_str()));
        match severity {
            EventSeverity::Error => web_sys::console::error_1(&line),
            EventSeverity::Warning => web_sys::console::warn_1(&line),
            _ => web_sys::console::log_1(&line),
        }
        true
    }
}

/// The time `budget_ms` milliseconds from now.
fn deadline(budget_ms: f64) -> Instant {
    Instant::now() + Duration::from_secs_f64(budget_ms.max(0.0) / 1000.0)
}

fn js_error(message: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&message.to_string()).into()
}
