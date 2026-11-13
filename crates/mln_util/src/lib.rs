//! Utilities shared by the other crates: geometry, math, colors, images,
//! time, strings, URLs, logging, threading helpers and more, one module per
//! file.

mod color;
pub mod compression;
pub mod constants;
pub mod geo;
pub mod interpolate;
mod padding;
pub mod range;
pub mod string;
mod unitbezier;

pub use color::{Color, ColorSpace};
pub use error::{Error, ErrorKind, ParseEnumError};
pub use geo::{EdgeInsets, LatLng, LatLngBounds};
pub use padding::Padding;
pub use unitbezier::UnitBezier;
pub mod bounding_volumes;
pub mod chrono;
pub mod error;
pub mod event_queue;
pub mod feature;
pub mod font_stack;
pub mod geometry;
pub mod grid_index;
#[cfg(target_family = "wasm")]
pub mod host_bytes;
pub mod http_header;
pub mod i18n;
pub mod identity;
pub mod image;
pub mod intersection_tests;
pub mod logging;
pub mod longest_common_subsequence;
pub mod mat3;
pub mod mat4;
pub mod math;
pub mod premultiply;
pub mod projection;
pub mod quaternion;
pub mod serialization;
pub mod size;
pub mod subdivision_granularity;
pub mod thread;
pub mod tileset;
pub mod time;
pub mod tiny_sdf;
pub mod token;
pub mod url;
pub mod vectors;

#[cfg(test)]
mod tests;
