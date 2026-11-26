//! The engine's utilities: the camera's geometry and math, threads, task
//! queues and pools, HTTP headers and dates, compression, SDF glyphs and
//! more, one module per file. The vocabulary the engine shares with the
//! style, such as errors, colors, sizes, coordinates and tilesets, is
//! `mln_style`'s.

pub mod compression;
pub mod constants;
pub mod dem;
pub mod geo;

pub use geo::EdgeInsets;
pub mod bounding_volumes;
pub mod chrono;
pub mod event_queue;
pub mod feature;
pub mod font_stack;
pub mod http_header;
pub mod identity;
pub mod intersection_tests;
pub mod math;
pub mod panic;
pub mod pool;
pub mod premultiply;
pub mod projection;
pub mod queue;
pub mod thread;
pub mod tiny_sdf;
pub mod token;

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) const GENERAL: &str = "mln_util::general";
}
