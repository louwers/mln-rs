//! Ports `src/mln/style/expression` + `include/mln/style/expression`.
//!
//! Module-per-file, mirroring the C++ tree. `type`, `match`, `let` and `in`
//! are Rust keywords, so those modules are `type_`, `match_`, `let_` and
//! `in_`.

pub mod assertion;
pub mod at;
pub mod boolean_operator;
pub mod case;
pub mod check_subtype;
pub mod coalesce;
pub mod coercion;
pub mod collator;
pub mod collator_expression;
pub mod comparison;
pub mod compound_expression;
pub mod distance;
pub mod dsl;
pub mod error;
pub mod expression;
pub mod find_zoom_curve;
pub mod format_expression;
pub mod formatted;
pub mod image;
pub mod image_expression;
pub mod in_;
pub mod index_of;
pub mod interpolate;
pub mod interpolator;
pub mod is_constant;
pub mod is_expression;
pub mod length;
pub mod let_;
pub mod literal;
pub mod match_;
pub mod number_format;
pub mod parsing_context;
pub mod range;
pub mod semiliteral;
pub mod slice;
pub mod step;
pub mod stops;
pub mod type_;
pub mod utf8_op_helpers;
pub mod util;
pub mod value;
pub mod within;

pub use expression::{
    Dependency, EvalResult, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind,
    downcast,
};
pub use find_zoom_curve::{ZoomCurve, ZoomCurvePtr, find_zoom_curve, find_zoom_curve_checked};
pub use formatted::{Formatted, FormattedSection};
pub use image::Image;
pub use interpolate::{Interpolate, create_interpolate};
pub use interpolator::Interpolator;
pub use is_constant::{feature_property_keys, is_feature_constant, is_runtime_constant, is_zoom_constant};
pub use is_expression::is_expression;
pub use literal::Literal;
pub use parsing_context::{ParseResult, ParsingContext, ParsingError, TypeAnnotationOption};
pub use step::Step;
pub use stops::Stops;
pub use type_::Type;
pub use value::{Value, ValueConverter, from_expression_value, to_expression_value, value_type_to_expression_type};

#[cfg(test)]
mod tests;
