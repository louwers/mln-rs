//! Style conversion: `Error`, the type-erased `Convertible` view of a
//! JSON-like value, its front ends over `serde_json` values, over
//! `serde_json_borrow` values and over `feature::Value`, `get_json_type`,
//! and the typed converters from a `Convertible` to style types.
//!
//! `Converter` is a trait for the converters of constants, which take no
//! arguments besides the value and the error. The converters of what can
//! hold an expression are free functions that take the expression functions
//! it parses against, such as `convert_filter(value, functions, error)` and
//! `convert_property_value::<T>(value, error, allow_data_expressions,
//! convert_tokens, functions)`.

pub(crate) mod borrowed;
pub(crate) mod color_ramp_property_value;
pub(crate) mod constant;
pub(crate) mod convertible;
pub(crate) mod coordinate;
pub(crate) mod filter;
pub mod function;
mod get_json_type;
pub(crate) mod image;
mod json_conversion;
pub mod layer;
pub(crate) mod light;
pub(crate) mod position;
pub(crate) mod projection;
pub(crate) mod property_value;
pub(crate) mod rotation;
pub(crate) mod sky;
pub mod source;
pub(crate) mod sprite;
pub(crate) mod transition_options;
mod value;

pub use convertible::{Convertible, Error, to_geojson};
pub use get_json_type::get_json_type;
pub use value::{json_to_value, value_to_json};

/// A conversion from a `Convertible` that takes no arguments besides the value
/// and the error.
pub trait Converter: Sized {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self>;
}

pub fn convert<T: Converter>(value: &Convertible<'_>, error: &mut Error) -> Option<T> {
    T::convert(value, error)
}

pub use color_ramp_property_value::convert_color_ramp_property_value;
pub use filter::convert_filter;
pub use layer::convert_layer;
pub use light::convert_light;
pub use projection::convert_projection;
pub use property_value::convert_property_value;
pub use sky::convert_sky;
pub use source::convert_source;
