//! The property system: a property's value as the style sets it
//! ([`value`], [`expression`], [`color_ramp`]), its conversion from style
//! JSON ([`convert`], with legacy functions in [`function`]), how it
//! transitions ([`transition`]) and evaluates for a frame ([`evaluation`],
//! [`evaluated`]), the property sets of a layer type, the light and the sky
//! ([`set`]), and access to them by style-spec name ([`table`]).

pub mod color_ramp;
pub mod convert;
pub mod evaluated;
pub mod evaluation;
pub mod expression;
pub mod function;
pub mod set;
#[doc(hidden)]
pub mod table;
pub mod transition;
pub mod value;
