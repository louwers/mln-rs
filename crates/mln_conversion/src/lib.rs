//! The core of style conversion: `Error`, the type-erased `Convertible` view
//! of a JSON-like value, its front ends over `serde_json` values and over
//! `feature::Value` (`value_conversion.rs`), and `get_json_type`.
//!
//! The typed converters (`Converter<T>`) live in `mln_style`.

mod conversion;
pub mod get_json_type;
mod json_conversion;
pub mod value_conversion;

pub use conversion::{Convertible, Error, to_geojson};
pub use get_json_type::get_json_type;
pub use value_conversion::{json_to_value, value_to_json};
