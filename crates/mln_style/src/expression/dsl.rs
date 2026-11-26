// Functions that build expressions for tests: they check little of what they
// build. Dependents' tests reach them through the `dsl` feature.

use crate::color::ColorSpace;
use crate::expression::Expr;
use crate::expression::formatted::Formatted;
use crate::expression::forms::assertion::Assertion;
use crate::expression::forms::coercion::Coercion;
use crate::expression::forms::comparison::Comparison;
use crate::expression::forms::compound::create_compound_expression;
use crate::expression::forms::format::{FormatExpression, FormatExpressionSection};
use crate::expression::forms::image::ImageExpression;
use crate::expression::forms::interpolate::create_interpolate;
use crate::expression::forms::step::Step;
use crate::expression::functions::Functions;
use crate::expression::interpolator::{ExponentialInterpolator, Interpolator};
use crate::expression::parse::ParsingContext;
use crate::expression::stops::Stops;
use crate::expression::type_::Type;
use crate::expression::value::Value;

/// The call of the built-in function `op` with `args`, for the calls this
/// module makes, which are always valid.
fn compound(op: &str, args: Vec<Expr>) -> Expr {
    let mut ctx = ParsingContext::new(&Functions::with_builtins());
    create_compound_expression(op, args, &mut ctx).expect("the dsl builds only valid built-in calls")
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

/// An `interpolate` expression with stops as (input, output) pairs, or
/// `None` when the input isn't a number, there are no stops, or they can't be
/// interpolated.
#[must_use]
pub fn interpolate(interpolator: Interpolator, input: Expr, stops_in: Vec<(f64, Expr)>) -> Option<Expr> {
    if input.get_type() != Type::Number {
        return None;
    }
    let type_ = stops_in.first()?.1.get_type();
    let mut stops = Stops::new();
    for (k, v) in stops_in {
        stops.insert_or_assign(k, v);
    }
    let mut ctx = ParsingContext::without_functions();
    create_interpolate(type_, interpolator, ColorSpace::Rgb, input, stops, &mut ctx)
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
