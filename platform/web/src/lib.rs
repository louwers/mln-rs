//! The browser build of mln-rs: the libraries built for
//! wasm32-unknown-unknown (no atomics) and driven from JavaScript through
//! wasm-bindgen. The page loads one wasm module, and Web Workers instantiate
//! it, each instance with its own memory:
//!
//! - In the map's Web Worker, `WebMap` (map.rs): the map, its renderer
//!   (wgpu's WebGPU backend, into the page's canvas as an OffscreenCanvas)
//!   and its file sources (fetch, fetch.rs). Each animation frame runs the
//!   RunLoop, draws, and runs the cooperative scheduler's tasks until the
//!   frame's budget is spent; between frames, the RunLoop and the scheduler
//!   wake the worker to run what arrives (`WebMap::tick`).
//! - In each tile worker's Web Worker, `TileWorker` (worker.rs): a
//!   `TileWorkerHost` on the instance's cooperative scheduler. The map's
//!   `SerializingTransport` puts each tile's worker (geometry, raster or
//!   raster-dem) in one of them and talks to it only through bytes, over a
//!   MessageChannel between the two Web Workers.
//!
//! The page's thread runs no wasm: it only starts the Web Workers and
//! forwards to the map's.
//!
//! An instance has no threads, so `start` installs a `CooperativeScheduler`
//! before anything else runs, and the instance's tasks run when its Web
//! Worker runs them.
//!
//! web/main.js, web/map-worker.js and web/worker.js are the JavaScript side.

#![cfg(target_arch = "wasm32")]

mod fetch;
mod map;
mod worker;

pub use map::WebMap;
pub use worker::TileWorker;

use std::sync::Arc;
use std::time::Duration;

use mln_actor::cooperative_scheduler::CooperativeScheduler;
use mln_actor::scheduler;
use mln_util::enum_::Enum;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_util::time::Instant;
use wasm_bindgen::prelude::*;

/// Runs when an instance starts, in each Web Worker, before any
/// map or file source exists (they would start threads otherwise).
#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&JsValue::from_str(&info.to_string()))));
    Log::set_observer(Box::new(ConsoleLog));
    scheduler::install_cooperative(Arc::new(CooperativeScheduler::new()));
}

/// The scheduler `start` installed.
fn cooperative_scheduler() -> Arc<CooperativeScheduler> {
    scheduler::cooperative().expect("installed at start").clone()
}

/// The port's log records go to the console.
struct ConsoleLog;

impl Observer for ConsoleLog {
    fn on_record(&self, severity: EventSeverity, event: Event, _code: i64, msg: &str) -> bool {
        let line = JsValue::from_str(&format!("[{}] {msg}", event.to_string()));
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
