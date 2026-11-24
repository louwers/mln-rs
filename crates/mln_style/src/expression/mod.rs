//! Style expressions: their types and values, parsing, and evaluation.
//!
//! A parsed expression is an [`Expr`], a closed tree whose nodes the parser
//! has type-checked, so that it evaluates typed (see [`expr`]). The
//! expression forms (`let`, `match`, `interpolate` and the others that parse
//! their own arguments) are a closed set of nodes; every other operator is a
//! function, which parsing resolves by name against a [`Functions`] value
//! (see [`functions`]). Each kind of node has its own module. `type`,
//! `match`, `let` and `in` are Rust keywords, so those modules are `type_`,
//! `match_`, `let_` and `in_`.

mod assertion;
mod at;
mod boolean_operator;
pub mod case;
mod check_subtype;
mod coalesce;
mod coercion;
mod collator;
mod collator_expression;
mod comparison;
pub mod compound_expression;
mod distance;
#[cfg(any(test, feature = "dsl"))]
pub mod dsl;
pub mod expr;
mod feature_property_keys;
mod find_zoom_curve;
pub mod format_expression;
pub mod format_section_override;
pub mod formatted;
pub mod functions;
pub mod image;
pub mod image_expression;
mod in_;
mod index_of;
mod interpolate;
mod interpolator;
pub mod is_expression;
mod length;
mod let_;
mod literal;
mod locale;
pub mod match_;
mod number_format;
pub mod parsing_context;
pub mod range;
mod semiliteral;
mod slice;
mod step;
mod stops;
pub mod type_;
mod utf8_op_helpers;
mod util;
pub mod value;
mod within;

pub use expr::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expr, GlobalStateMap, collect_global_state_refs,
    global_state_refs_intersect,
};
pub use feature_property_keys::feature_property_keys;
pub use find_zoom_curve::{ZoomCurvePtr, find_zoom_curve_checked};
pub use formatted::{Formatted, FormattedSection};
pub use functions::Functions;
pub use image::Image;
pub use interpolate::Interpolate;
pub use interpolator::Interpolator;
pub use is_expression::is_expression;
// For the test runners, whose expectations assume a default locale.
#[doc(hidden)]
pub use locale::set_default_locale;
pub use parsing_context::{ParseResult, ParsingContext, TypeAnnotationOption};
pub use step::Step;
pub use stops::Stops;
pub use type_::Type;
pub use value::{Value, ValueConverter, from_expression_value};

#[cfg(test)]
mod tests;
