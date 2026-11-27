//! Style conversion: `Error`, the type-erased `Convertible` view of a
//! JSON-like value, its front ends over `serde_json` values, over
//! `serde_json_borrow` values (the style parser's) and over
//! `crate::feature::Value`, `get_json_type`, and the converters of value types.
//!
//! `Converter` is a trait for the converters of constants, which take no
//! arguments besides the value and the error. A structure that can hold an
//! expression has its converter next to it, a free function that takes the
//! expression functions it parses against, such as
//! `filter::convert_filter(value, functions, error)` and
//! `property::convert::convert_property_value::<T>(value, error,
//! allow_data_expressions, convert_tokens, functions)`.

pub(crate) mod borrowed;
pub(crate) mod constant;
mod convertible;
pub(crate) mod coordinate;
pub(crate) mod geojson;
pub(crate) mod geojson_options;
mod get_json_type;
pub(crate) mod image;
mod json_conversion;
pub(crate) mod position;
pub(crate) mod rotation;
pub(crate) mod source;
pub(crate) mod sprite;
pub mod tileset;
pub(crate) mod to_value;
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

pub use geojson_options::convert_geojson_options;
