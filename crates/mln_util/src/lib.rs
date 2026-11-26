//! Utilities shared by the other crates: geometry, math, colors, images,
//! time, strings, URLs, logging, threading helpers and more, one module per
//! file.

#[cfg(target_os = "android")]
pub mod android;
mod color;
pub mod compression;
pub mod constants;
pub mod dem;
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
pub mod clip_depth;
pub mod error;
pub mod event_queue;
pub mod feature;
pub mod font_stack;
pub mod geometry;
pub mod hash;
pub mod http_header;
pub mod i18n;
pub mod identity;
pub mod image;
pub mod intersection_tests;
pub mod logging;
pub mod math;
pub mod panic;
pub mod pool;
pub mod premultiply;
pub mod projection;
pub mod queue;
pub mod size;
pub mod subdivision_granularity;
pub mod thread;
pub mod tileset;
pub mod time;
pub mod tiny_sdf;
pub mod token;

#[cfg(test)]
mod tests;
