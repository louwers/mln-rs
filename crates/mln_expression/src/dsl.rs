// Ports include/mln/style/expression/dsl.hpp,
// src/mln/style/expression/dsl_impl.hpp and
// src/mln/style/expression/dsl.cpp.
//
// This convenience API does little to no expression validation or
// type-checking, and is intended for use only by test and other non-production
// code (and the legacy function conversion).

use mln_conversion::Convertible;
use mln_util::ColorSpace;

use crate::assertion::Assertion;
use crate::coercion::Coercion;
use crate::comparison::BasicComparison;
use crate::compound_expression::create_compound_expression;
use crate::error::Error;
use crate::expression::Expression;
use crate::format_expression::{FormatExpression, FormatExpressionSection};
use crate::formatted::Formatted;
use crate::image::Image;
use crate::image_expression::ImageExpression;
use crate::interpolate::create_interpolate;
use crate::interpolator::{CubicBezierInterpolator, ExponentialInterpolator, Interpolator};
use crate::literal::Literal;
use crate::parsing_context::ParsingContext;
use crate::step::Step;
use crate::stops::Stops;
use crate::type_::Type;
use crate::value::Value;

pub fn compound(op: &str, args: Vec<Box<dyn Expression>>) -> Box<dyn Expression> {
    let mut ctx = ParsingContext::new();
    let result = create_compound_expression(op, args, &mut ctx);
    result.expect("dsl::compound")
}

/// `dsl::createExpression(const char*)`: parses JSON text.
pub fn create_expression_from_str(expr: &str) -> Option<Box<dyn Expression>> {
    // TODO(port): rapidjson `Parse<0>`; serde_json accepts the same JSON.
    let document: serde_json::Value = serde_json::from_str(expr).ok()?;
    let mut ctx = ParsingContext::new();
    ctx.parse_expression(&Convertible::from(&document), None)
}

pub fn create_expression(expr: &Convertible<'_>) -> Option<Box<dyn Expression>> {
    let mut ctx = ParsingContext::new();
    ctx.parse_expression(expr, None)
}

pub fn error(message: impl Into<String>) -> Box<dyn Expression> {
    Box::new(Error::new(message.into()))
}

pub fn literal_str(value: &str) -> Box<dyn Expression> {
    literal(Value::String(value.to_owned()))
}

pub fn literal(value: Value) -> Box<dyn Expression> {
    Box::new(Literal::new(value))
}

pub fn literal_numbers(value: &[f64]) -> Box<dyn Expression> {
    literal(Value::Array(value.iter().map(|v| Value::Number(*v)).collect()))
}

pub fn literal_strings(value: &[&str]) -> Box<dyn Expression> {
    literal(Value::Array(value.iter().map(|v| Value::String((*v).to_owned())).collect()))
}

pub fn assertion(type_: Type, value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    let mut v = vec![value];
    if let Some(def) = def {
        v.push(def);
    }
    Box::new(Assertion::new(type_, v))
}

pub fn number(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    assertion(Type::Number, value, def)
}

pub fn string(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    assertion(Type::String, value, def)
}

pub fn boolean(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    assertion(Type::Boolean, value, def)
}

fn coercion(type_: Type, value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    let mut v = vec![value];
    if let Some(def) = def {
        v.push(def);
    }
    Box::new(Coercion::new(type_, v))
}

pub fn to_color(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::Color, value, def)
}

pub fn to_padding(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::Padding, value, def)
}

pub fn to_projection_definition(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::ProjectionDefinition, value, def)
}

pub fn to_variable_anchor_offset(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::VariableAnchorOffsetCollection, value, def)
}

pub fn to_string(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::String, value, def)
}

pub fn to_formatted(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::Formatted, value, def)
}

pub fn to_image(value: Box<dyn Expression>, def: Option<Box<dyn Expression>>) -> Box<dyn Expression> {
    coercion(Type::Image, value, def)
}

pub fn get_str(value: &str) -> Box<dyn Expression> {
    get(literal_str(value))
}

pub fn get(property: Box<dyn Expression>) -> Box<dyn Expression> {
    compound("get", vec![property])
}

pub fn id() -> Box<dyn Expression> {
    compound("id", vec![])
}

pub fn zoom() -> Box<dyn Expression> {
    compound("zoom", vec![])
}

pub fn eq(lhs: Box<dyn Expression>, rhs: Box<dyn Expression>) -> Box<dyn Expression> {
    Box::new(BasicComparison::new("==".to_owned(), lhs, rhs))
}

pub fn ne(lhs: Box<dyn Expression>, rhs: Box<dyn Expression>) -> Box<dyn Expression> {
    Box::new(BasicComparison::new("!=".to_owned(), lhs, rhs))
}

pub fn gt(lhs: Box<dyn Expression>, rhs: Box<dyn Expression>) -> Box<dyn Expression> {
    Box::new(BasicComparison::new(">".to_owned(), lhs, rhs))
}

pub fn lt(lhs: Box<dyn Expression>, rhs: Box<dyn Expression>) -> Box<dyn Expression> {
    Box::new(BasicComparison::new("<".to_owned(), lhs, rhs))
}

pub fn step(
    input: Box<dyn Expression>,
    output0: Box<dyn Expression>,
    input1: f64,
    output1: Box<dyn Expression>,
) -> Box<dyn Expression> {
    let type_ = output0.get_type().clone();
    let mut stops = Stops::new();
    stops.insert_or_assign(f64::NEG_INFINITY, output0);
    stops.insert_or_assign(input1, output1);
    Box::new(Step::new(type_, input, stops))
}

pub fn linear() -> Interpolator {
    ExponentialInterpolator::new(1.0).into()
}

pub fn exponential(base: f64) -> Interpolator {
    ExponentialInterpolator::new(base).into()
}

pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64) -> Interpolator {
    CubicBezierInterpolator::new(x1, y1, x2, y2).into()
}

/// The `dsl::interpolate` overloads: stops as (input, output) pairs.
pub fn interpolate(
    interpolator: Interpolator,
    input: Box<dyn Expression>,
    stops_in: Vec<(f64, Box<dyn Expression>)>,
) -> Box<dyn Expression> {
    let type_ = stops_in.first().expect("at least one stop").1.get_type().clone();
    let mut stops = Stops::new();
    for (k, v) in stops_in {
        stops.insert_or_assign(k, v);
    }
    let mut ctx = ParsingContext::new();
    let result = create_interpolate(type_, interpolator, ColorSpace::Rgb, input, stops, &mut ctx);
    result.expect("dsl::interpolate")
}

pub fn concat(inputs: Vec<Box<dyn Expression>>) -> Box<dyn Expression> {
    compound("concat", inputs)
}

pub fn format_str(value: &str) -> Box<dyn Expression> {
    Box::new(Literal::new(Value::Formatted(Formatted::from_plain(value))))
}

pub fn format(input: Box<dyn Expression>) -> Box<dyn Expression> {
    let sections = vec![FormatExpressionSection::new(input)];
    Box::new(FormatExpression::new(sections))
}

pub fn image_str(value: &str) -> Box<dyn Expression> {
    Box::new(Literal::new(Value::Image(Image::new(value))))
}

pub fn image(expression: Box<dyn Expression>) -> Box<dyn Expression> {
    Box::new(ImageExpression::new(expression))
}
