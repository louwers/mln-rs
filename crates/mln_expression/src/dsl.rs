// This convenience API does little to no expression validation or
// type-checking, and is intended for use only by test and other non-production
// code (and the legacy function conversion).

use mln_conversion::Convertible;
use mln_util::ColorSpace;

use crate::assertion::Assertion;
use crate::coercion::Coercion;
use crate::comparison::Comparison;
use crate::compound_expression::create_compound_expression;
use crate::expression::Expr;
use crate::format_expression::{FormatExpression, FormatExpressionSection};
use crate::formatted::Formatted;
use crate::image_expression::ImageExpression;
use crate::interpolate::create_interpolate;
use crate::interpolator::{ExponentialInterpolator, Interpolator};
use crate::parsing_context::ParsingContext;
use crate::step::Step;
use crate::stops::Stops;
use crate::type_::Type;
use crate::value::Value;

#[must_use]
pub fn compound(op: &str, args: Vec<Expr>) -> Expr {
    let mut ctx = ParsingContext::new();
    let result = create_compound_expression(op, args, &mut ctx);
    result.expect("dsl::compound")
}

/// Parses an expression from JSON text.
#[must_use]
pub fn create_expression_from_str(expr: &str) -> Option<Expr> {
    let document: serde_json::Value = serde_json::from_str(expr).ok()?;
    let mut ctx = ParsingContext::new();
    ctx.parse_expression(&Convertible::from(&document), None)
}

#[must_use]
pub fn create_expression(expr: &Convertible<'_>) -> Option<Expr> {
    let mut ctx = ParsingContext::new();
    ctx.parse_expression(expr, None)
}

/// `["error", message]`.
pub fn error(message: impl Into<String>) -> Expr {
    compound("error", vec![literal(Value::String(message.into()))])
}

#[must_use]
pub fn literal_str(value: &str) -> Expr {
    literal(Value::String(value.to_owned()))
}

#[must_use]
pub fn literal(value: Value) -> Expr {
    Expr::literal(value)
}

#[must_use]
pub fn assertion(type_: Type, value: Expr, def: Option<Expr>) -> Expr {
    let mut v = vec![value];
    if let Some(def) = def {
        v.push(def);
    }
    Assertion::new_expr(type_, v)
}

#[must_use]
pub fn number(value: Expr, def: Option<Expr>) -> Expr {
    assertion(Type::Number, value, def)
}

#[must_use]
pub fn string(value: Expr, def: Option<Expr>) -> Expr {
    assertion(Type::String, value, def)
}

#[must_use]
pub fn boolean(value: Expr, def: Option<Expr>) -> Expr {
    assertion(Type::Boolean, value, def)
}

fn coercion(type_: Type, value: Expr, def: Option<Expr>) -> Expr {
    let mut v = vec![value];
    if let Some(def) = def {
        v.push(def);
    }
    Coercion::new_expr(type_, v)
}

#[must_use]
pub fn to_color(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::Color, value, def)
}

#[must_use]
pub fn to_padding(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::Padding, value, def)
}

#[must_use]
pub fn to_projection_definition(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::ProjectionDefinition, value, def)
}

#[must_use]
pub fn to_variable_anchor_offset(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::VariableAnchorOffsetCollection, value, def)
}

#[must_use]
pub fn to_string(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::String, value, def)
}

#[must_use]
pub fn to_formatted(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::Formatted, value, def)
}

#[must_use]
pub fn to_image(value: Expr, def: Option<Expr>) -> Expr {
    coercion(Type::Image, value, def)
}

#[must_use]
pub fn get_str(value: &str) -> Expr {
    get(literal_str(value))
}

#[must_use]
pub fn get(property: Expr) -> Expr {
    compound("get", vec![property])
}

#[must_use]
pub fn id() -> Expr {
    compound("id", vec![])
}

#[must_use]
pub fn zoom() -> Expr {
    compound("zoom", vec![])
}

#[must_use]
pub fn eq(lhs: Expr, rhs: Expr) -> Expr {
    Comparison::new_expr("==", lhs, rhs, None)
}

#[must_use]
pub fn ne(lhs: Expr, rhs: Expr) -> Expr {
    Comparison::new_expr("!=", lhs, rhs, None)
}

#[must_use]
pub fn gt(lhs: Expr, rhs: Expr) -> Expr {
    Comparison::new_expr(">", lhs, rhs, None)
}

#[cfg(test)]
#[must_use]
pub(crate) fn lt(lhs: Expr, rhs: Expr) -> Expr {
    Comparison::new_expr("<", lhs, rhs, None)
}

#[must_use]
pub fn step(input: Expr, output0: Expr, input1: f64, output1: Expr) -> Expr {
    let type_ = output0.get_type();
    let mut stops = Stops::new();
    stops.insert_or_assign(f64::NEG_INFINITY, output0);
    stops.insert_or_assign(input1, output1);
    Step::new_expr(type_, input, stops)
}

#[must_use]
pub fn linear() -> Interpolator {
    ExponentialInterpolator::new(1.0).into()
}

#[must_use]
pub fn exponential(base: f64) -> Interpolator {
    ExponentialInterpolator::new(base).into()
}

/// The `dsl::interpolate` overloads: stops as (input, output) pairs.
#[must_use]
pub fn interpolate(interpolator: Interpolator, input: Expr, stops_in: Vec<(f64, Expr)>) -> Expr {
    let type_ = stops_in.first().expect("at least one stop").1.get_type();
    let mut stops = Stops::new();
    for (k, v) in stops_in {
        stops.insert_or_assign(k, v);
    }
    let mut ctx = ParsingContext::new();
    let result = create_interpolate(type_, interpolator, ColorSpace::Rgb, input, stops, &mut ctx);
    result.expect("dsl::interpolate")
}

#[must_use]
pub fn concat(inputs: Vec<Expr>) -> Expr {
    compound("concat", inputs)
}

#[must_use]
pub fn format_str(value: &str) -> Expr {
    Expr::literal(Value::Formatted(Formatted::from_plain(value)))
}

#[must_use]
pub fn format(input: Expr) -> Expr {
    let sections = vec![FormatExpressionSection::new(input)];
    FormatExpression::new_expr(sections)
}

#[must_use]
pub fn image(expression: Expr) -> Expr {
    ImageExpression::new_expr(expression)
}
