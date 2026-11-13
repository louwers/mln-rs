//! Style expressions: their types and values, parsing, and evaluation.
//!
//! Each expression has its own module. `type`, `match`, `let` and `in` are
//! Rust keywords, so those modules are `type_`, `match_`, `let_` and `in_`.

mod assertion;
mod at;
pub mod boolean_operator;
pub mod case;
mod check_subtype;
mod coalesce;
mod coercion;
mod collator;
mod collator_expression;
mod comparison;
pub mod compound_expression;
mod distance;
pub mod dsl;
mod error;
pub mod expression;
mod find_zoom_curve;
pub mod format_expression;
pub mod formatted;
pub mod image;
pub mod image_expression;
mod in_;
mod index_of;
mod interpolate;
mod interpolator;
mod is_constant;
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

pub use expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
pub use find_zoom_curve::{ZoomCurvePtr, find_zoom_curve_checked};
pub use formatted::{Formatted, FormattedSection};
pub use image::Image;
pub use interpolate::{Interpolate, create_interpolate};
pub use interpolator::Interpolator;
pub use is_constant::{feature_property_keys, is_feature_constant, is_runtime_constant, is_zoom_constant};
pub use is_expression::is_expression;
pub use literal::Literal;
pub use parsing_context::{ParseResult, ParsingContext, TypeAnnotationOption};
pub use step::Step;
pub use stops::Stops;
pub use type_::Type;
pub use value::{Value, ValueConverter, from_expression_value};

#[cfg(test)]
mod tests;
