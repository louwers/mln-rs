//! Utilities shared by the other crates: geometry, math, images, time,
//! strings, URLs, threading helpers and more, one module per file.

pub mod compression;
pub mod constants;
pub mod dem;
pub mod geo;
pub mod range;
pub mod string;
mod unitbezier;

pub use error::{Error, ErrorKind, ParseEnumError};
pub use geo::{EdgeInsets, LatLng, LatLngBounds};
pub use unitbezier::UnitBezier;
pub mod bounding_volumes;
pub mod chrono;
pub mod error;
pub mod event_queue;
pub mod feature;
pub mod font_stack;
pub mod hash;
pub mod http_header;
pub mod identity;
pub mod image;
pub mod intersection_tests;
pub mod math;
pub mod panic;
pub mod pool;
pub mod premultiply;
pub mod projection;
pub mod queue;
pub mod size;
pub mod thread;
pub mod tileset;
pub mod time;
pub mod tiny_sdf;
pub mod token;

/// The `log` targets of this crate's records. Each names the event that
/// `mln_c` reports the record under (`mln_c::logging`).
mod log_target {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) const GENERAL: &str = "mln_util::general";
}

#[cfg(test)]
mod tests;
