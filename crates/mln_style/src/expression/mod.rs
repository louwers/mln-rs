//! Style expressions: their types and values, parsing, and evaluation.
//!
//! A parsed expression is an [`Expr`], a closed tree whose nodes the parser
//! has type-checked, so that it evaluates typed. The expression forms (`let`,
//! `match`, `interpolate` and the others that parse their own arguments) are
//! a closed set of nodes, one module each under `forms`; every other operator
//! is a function, which parsing resolves by name against a [`Functions`]
//! value (see [`functions`]). `type`, `match`, `let` and `in` are Rust
//! keywords, so those modules are `type_`, `match_`, `let_` and `in_`.

mod dependency;
#[cfg(any(test, feature = "dsl"))]
pub mod dsl;
mod evaluate;
pub mod formatted;
pub(crate) mod forms;
pub mod functions;
pub mod image;
mod interpolator;
mod locale;
mod node;
pub(crate) mod parse;
mod stops;
pub mod type_;
pub mod value;
mod zoom_curve;

pub use dependency::{Dependency, collect_global_state_refs, feature_property_keys, global_state_refs_intersect};
#[cfg(test)]
pub(crate) use evaluate::evaluate_feature;
pub(crate) use evaluate::mbgl_to_expression_value;
pub use evaluate::{
    EvalResult, EvaluationContext, EvaluationError, EvaluationResult, GlobalStateMap, evaluate_accumulated,
    evaluate_feature_with_context, with_geojson_feature,
};
pub use formatted::{Formatted, FormattedSection};
pub use forms::case::Case;
pub use forms::compound::create_compound_expression;
pub use forms::format::{FormatExpression, FormatExpressionSection};
pub use forms::format_section_override::{FormatSectionOverride, OverrideDefault};
pub use forms::image::{ImageExpression, UnavailableImages};
pub use forms::interpolate::Interpolate;
pub use forms::match_::{InputType, Labels, Match, MatchLabel};
pub use forms::step::Step;
pub use functions::Functions;
pub use image::Image;
pub use interpolator::Interpolator;
// For the test runners, whose expectations assume a default locale.
#[doc(hidden)]
pub use locale::set_default_locale;
pub use node::Expr;
pub(crate) use node::{Node, Output, SharedExpr};
pub use parse::{
    ParseResult, ParsingContext, ParsingError, TypeAnnotationOption, create_expression, create_expression_from_str,
    is_expression,
};
pub use stops::Stops;
pub use type_::Type;
pub use value::{Value, ValueConverter, from_expression_value};
pub use zoom_curve::{ZoomCurvePtr, find_zoom_curve_checked};

#[cfg(test)]
mod tests;
