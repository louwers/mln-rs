//! Ports the typed converters of `mln::style::conversion`
//! (include/mln/style/conversion/*.hpp, src/mln/style/conversion/*.cpp).
//! `Convertible` and `Error` come from `mln_conversion`.
//!
//! C++ dispatches `convert<T>(value, error, args...)` to
//! `Converter<T>::operator()`. Here `Converter` is a trait for the
//! converters without extra arguments; the ones with extra arguments are
//! free functions: `convert_property_value::<T>(value, error,
//! allow_data_expressions, convert_tokens)` and
//! `convert_source(value, error, id)`.

pub mod color_ramp_property_value;
pub mod constant;
pub mod coordinate;
pub mod filter;
pub mod function;
pub mod geojson;
pub mod geojson_options;
pub mod image;
pub mod layer;
pub mod light;
pub mod position;
pub mod projection;
pub mod property_value;
pub mod rotation;
pub mod sky;
pub mod source;
pub mod sprite;
pub mod stringify;
pub mod tileset;
pub mod transition_options;

pub use mln_conversion::{Convertible, Error};

/// `Converter<T>` (the specializations whose call operator takes only
/// `(value, error)`).
pub trait Converter: Sized {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self>;
}

/// `convert<T>(value, error)`.
pub fn convert<T: Converter>(value: &Convertible<'_>, error: &mut Error) -> Option<T> {
    T::convert(value, error)
}

pub use color_ramp_property_value::convert_color_ramp_property_value;
pub use layer::{convert_layer, set_paint_properties};
pub use property_value::convert_property_value;
pub use source::convert_source;
