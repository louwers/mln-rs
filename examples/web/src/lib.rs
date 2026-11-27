//! The browser build of mln-rs: the libraries built for
//! wasm32-unknown-unknown (no atomics) and driven from JavaScript through
//! wasm-bindgen. The page loads one wasm module, and Web Workers instantiate
//! it, each instance with its own memory:
//!
//! - In the map's Web Worker, `WebMap` (map.rs): a runtime (`mln_runtime`)
//!   and its map, whose render session presents to the page's canvas, an
//!   `OffscreenCanvas`, with the browser's WebGPU, or its WebGL2 in the
//!   `webgl` build. The runtime's and the session's work runs in the slices
//!   of `mln_runtime::web::pump`, and the page's animation frames demand frames.
//! - In each tile worker's Web Worker, `mln_runtime::web`'s tile worker side
//!   (`serve_tile_worker`): a `TileWorkerHost`, which runs the tiles' jobs:
//!   layouts, and raster and raster-dem tiles' image decodes. The session's
//!   renderer sends them only bytes, over a `MessageChannel` between the two
//!   Web Workers. Both sides make their registry with `registry`.
//! - In the map's Web Worker again, once a style declares font faces: the
//!   shaper module (`crates/mln_web_shaper`), which shapes and rasterizes them
//!   (`mln_runtime::web::shaper`). Only the map's Web Worker loads it, and maps
//!   without font faces never do.
//!
//! The page's thread runs no wasm: it only starts the Web Workers and
//! forwards to the map's.
//!
//! An instance has no threads, so the instance's work runs in the slices of
//! its pump (`mln_runtime::web::pump`) when its Web Worker's event loop runs
//! them.
//!
//! web/main.ts, web/map-worker.ts and web/tile-worker.ts are the page's side.

#![cfg(target_arch = "wasm32")]

mod gpu_check;
mod map;

use std::sync::Arc;

pub use map::{WebMap, set_preloads};
use mln_runtime::registry::{Registry, RegistryBuilder, register_builtins};
use wasm_bindgen::prelude::*;

/// Runs when an instance starts, in each Web Worker, before any map
/// exists.
#[wasm_bindgen(start)]
fn start() {
    // The Web Worker's script reads a panic's message from `mlnPanic`.
    mln_runtime::web::set_panic_hook();
    if log::set_logger(&ConsoleLog).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// The registrations of the map's runtime and of the tile workers, which
/// must agree.
fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    register_builtins(&mut builder).expect("the built-ins register into no registrations");
    builder.build()
}

/// Runs tile workers for the map side at the other end of `port`, until it
/// disconnects. web/tile-worker.ts calls it for each port that a map's Web
/// Worker sends it.
#[wasm_bindgen(js_name = mlnWebServeTileWorker)]
pub fn serve_tile_worker(port: &web_sys::MessagePort) {
    mln_runtime::web::tile_worker::serve(port, registry());
}

/// Panics with `message`. Only check/check.mjs calls it (the page's `panic`
/// parameter), to test that a panic stops the page.
#[wasm_bindgen]
pub fn panic_for_check(message: &str) {
    panic!("{message}");
}

/// The `log` records of the library and wgpu go to the console.
struct ConsoleLog;

impl log::Log for ConsoleLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = JsValue::from_str(&format!("[{}] {}", record.target(), record.args()));
        match record.level() {
            log::Level::Error => web_sys::console::error_1(&line),
            log::Level::Warn => web_sys::console::warn_1(&line),
            _ => web_sys::console::log_1(&line),
        }
    }

    fn flush(&self) {}
}

fn js_error(message: impl std::fmt::Display) -> JsValue {
    js_sys::Error::new(&message.to_string()).into()
}
