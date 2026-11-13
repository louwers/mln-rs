//! The typed converters from a `Convertible` (from `mln_conversion`) to style
//! types.
//!
//! `Converter` is a trait for the converters without extra arguments; the
//! ones with extra arguments are free functions:
//! `convert_property_value::<T>(value, error, allow_data_expressions,
//! convert_tokens)` and `convert_source(value, error, id)`.

pub(crate) mod color_ramp_property_value;
pub(crate) mod constant;
pub(crate) mod coordinate;
pub(crate) mod custom_geometry_source_options;
pub(crate) mod filter;
pub mod function;
pub(crate) mod geojson;
pub(crate) mod geojson_options;
pub(crate) mod image;
pub mod layer;
pub(crate) mod light;
pub(crate) mod position;
pub(crate) mod projection;
pub(crate) mod property_value;
pub(crate) mod rotation;
pub(crate) mod sky;
pub mod source;
pub(crate) mod sprite;
pub mod tileset;
pub(crate) mod transition_options;

pub use mln_conversion::{Convertible, Error};

/// A conversion from a `Convertible` that takes no arguments besides the value
/// and the error.
pub trait Converter: Sized {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self>;
}

pub fn convert<T: Converter>(value: &Convertible<'_>, error: &mut Error) -> Option<T> {
    T::convert(value, error)
}

pub(crate) use color_ramp_property_value::convert_color_ramp_property_value;
pub use layer::convert_layer;
pub use property_value::convert_property_value;
pub use source::convert_source;
