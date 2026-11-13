use std::sync::Arc;

use feature::Value as MbglValue;
use mln_expression::boolean_operator::{All, Any};
use mln_expression::compound_expression::create_compound_expression;

use super::{Converter, Convertible, Error};
use crate::expression::{Expression, Literal, ParseResult, ParsingContext, Type, Value};
use crate::filter::Filter;

impl Converter for Filter {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Filter> {
        if is_expression(value) {
            let mut parsing_context = ParsingContext::with_expected(Type::Boolean);
            let Some(parse_result) = parsing_context.parse_expression(value, None) else {
                error.message = parsing_context.get_combined_errors();
                return None;
            };
            Some(Filter::new(Some(Arc::from(parse_result)), None))
        } else {
            let Some(expression) = convert_legacy_filter(value, error) else {
                debug_assert_ne!(error.message, "");
                return None;
            };
            Some(Filter::new(Some(Arc::from(expression)), serialize_legacy_filter(value)))
        }
    }
}

// This is a port from
// https://github.com/mapbox/mapbox-gl-js/blob/master/src/style-spec/feature_filter/index.js
fn is_expression(filter: &Convertible<'_>) -> bool {
    if !filter.is_array() || filter.array_length() == 0 {
        return false;
    }

    let Some(op) = filter.array_member(0).to_string() else {
        return false;
    };

    if op == "has" {
        if filter.array_length() < 2 {
            return false;
        }
        let operand = filter.array_member(1).to_string();
        operand.is_some_and(|operand| operand != "$id" && operand != "$type")
    } else if op == "!in" || op == "!has" || op == "none" {
        false
    } else if op == "in" {
        let str_ = filter.array_member(1).to_string();

        filter.array_length() >= 3 && (str_.is_none() || filter.array_member(2).is_array())
    } else if op == "==" || op == "!=" || op == ">" || op == ">=" || op == "<" || op == "<=" {
        filter.array_length() != 3 || filter.array_member(1).is_array() || filter.array_member(2).is_array()
    } else if op == "any" || op == "all" {
        for i in 1..filter.array_length() {
            let f = filter.array_member(i);
            if !is_expression(&f) && f.to_bool().is_none() {
                return false;
            }
        }
        true
    } else {
        true
    }
}

fn create_expression(op: &str, args: Option<Vec<Box<dyn Expression>>>, error: &mut Error) -> ParseResult {
    let args = args?;

    if op == "any" {
        Some(Box::new(Any::new(args)))
    } else if op == "all" {
        Some(Box::new(All::new(args)))
    } else {
        let mut parsing_context = ParsingContext::with_expected(Type::Boolean);
        let parse_result = create_compound_expression(op, args, &mut parsing_context);
        if parse_result.is_none() {
            error.message = parsing_context.get_combined_errors();
        }
        parse_result
    }
}

fn create_expression_1(op: &str, arg: ParseResult, error: &mut Error) -> ParseResult {
    let arg = arg?;
    create_expression(op, Some(vec![arg]), error)
}

fn convert_literal(convertible: &Convertible<'_>, error: &mut Error) -> ParseResult {
    let mut parsing_context = ParsingContext::new();
    let parse_result = Literal::parse(convertible, &mut parsing_context);
    if parse_result.is_none() {
        error.message = parsing_context.get_combined_errors();
    }
    parse_result
}

fn convert_literal_array(
    input: &Convertible<'_>,
    error: &mut Error,
    start_index: usize,
) -> Option<Vec<Box<dyn Expression>>> {
    let mut output = Vec::with_capacity(input.array_length());
    for i in start_index..input.array_length() {
        let literal = convert_literal(&input.array_member(i), error)?;
        output.push(literal);
    }
    Some(output)
}

fn convert_legacy_comparison_filter(
    values: &Convertible<'_>,
    error: &mut Error,
    op_override: Option<&str>,
) -> ParseResult {
    let op = match op_override {
        Some(op) => Some(op.to_owned()),
        None => values.array_member(0).to_string(),
    };
    let property = values.array_member(1).to_string();

    let Some(property) = property else {
        error.message = "filter property must be a string".to_owned();
        return None;
    };
    // C++ dereferences `op` unchecked; the caller checked it is a string.
    let op = op.unwrap_or_default();
    if property == "$type" {
        let args = convert_literal_array(values, error, 2);
        create_expression(&format!("filter-type-{op}"), args, error)
    } else if property == "$id" {
        let args = convert_literal_array(values, error, 2);
        create_expression(&format!("filter-id-{op}"), args, error)
    } else {
        let args = convert_literal_array(values, error, 1);
        create_expression(&format!("filter-{op}"), args, error)
    }
}

fn convert_legacy_has_filter(values: &Convertible<'_>, error: &mut Error) -> ParseResult {
    let property = values.array_member(1).to_string();

    let Some(property) = property else {
        error.message = "filter property must be a string".to_owned();
        return None;
    };
    if property == "$type" {
        Some(Box::new(Literal::new(Value::Bool(true))))
    } else if property == "$id" {
        create_expression("filter-has-id", Some(Vec::new()), error)
    } else {
        create_expression("filter-has", Some(vec![Box::new(Literal::new(Value::String(property)))]), error)
    }
}

fn convert_legacy_in_filter(values: &Convertible<'_>, error: &mut Error) -> ParseResult {
    let property = values.array_member(1).to_string();

    let Some(property) = property else {
        error.message = "filter property must be a string".to_owned();
        return None;
    };
    if values.array_length() == 0 {
        Some(Box::new(Literal::new(Value::Bool(false))))
    } else if property == "$type" {
        let args = convert_literal_array(values, error, 2);
        create_expression("filter-type-in", args, error)
    } else if property == "$id" {
        let args = convert_literal_array(values, error, 2);
        create_expression("filter-id-in", args, error)
    } else {
        let args = convert_literal_array(values, error, 1);
        create_expression("filter-in", args, error)
    }
}

fn convert_legacy_filter_array(
    input: &Convertible<'_>,
    error: &mut Error,
    start_index: usize,
) -> Option<Vec<Box<dyn Expression>>> {
    let mut output = Vec::with_capacity(input.array_length());
    for i in start_index..input.array_length() {
        let child = convert_legacy_filter(&input.array_member(i), error)?;
        output.push(child);
    }
    Some(output)
}

pub(crate) fn convert_legacy_filter(values: &Convertible<'_>, error: &mut Error) -> ParseResult {
    if values.is_undefined() {
        return Some(Box::new(Literal::new(Value::Bool(true))));
    }

    if !values.is_array() || values.array_length() == 0 {
        error.message = "filter value must be a non empty array".to_owned();
        return None;
    }

    let Some(op) = values.array_member(0).to_string() else {
        error.message = "filter operator must be a string".to_owned();
        return None;
    };
    if values.array_length() <= 1 {
        return Some(Box::new(Literal::new(Value::Bool(op != "any"))));
    }
    if op == "within" {
        let mut ctx = ParsingContext::new();
        return ctx.parse_expression(values, None);
    }
    match op.as_str() {
        "==" | "<" | ">" | "<=" | ">=" => convert_legacy_comparison_filter(values, error, None),
        "!=" => {
            let inner = convert_legacy_comparison_filter(values, error, Some("=="));
            create_expression_1("!", inner, error)
        }
        "any" => {
            let args = convert_legacy_filter_array(values, error, 1);
            create_expression("any", args, error)
        }
        "all" => {
            let args = convert_legacy_filter_array(values, error, 1);
            create_expression("all", args, error)
        }
        "none" => {
            let args = convert_legacy_filter_array(values, error, 1);
            let any = create_expression("any", args, error);
            create_expression_1("!", any, error)
        }
        "in" => convert_legacy_in_filter(values, error),
        "!in" => {
            let inner = convert_legacy_in_filter(values, error);
            create_expression_1("!", inner, error)
        }
        "has" => convert_legacy_has_filter(values, error),
        "!has" => {
            let inner = convert_legacy_has_filter(values, error);
            create_expression_1("!", inner, error)
        }
        _ => Some(Box::new(Literal::new(Value::Bool(true)))),
    }
}

#[must_use]
pub(crate) fn serialize_legacy_filter(values: &Convertible<'_>) -> Option<MbglValue> {
    if values.is_undefined() {
        None
    } else if values.is_array() {
        let mut result = Vec::with_capacity(values.array_length());
        for i in 0..values.array_length() {
            let array_value = serialize_legacy_filter(&values.array_member(i));
            result.push(array_value.unwrap_or(MbglValue::Null));
        }
        Some(MbglValue::Array(result))
    } else {
        values.to_value()
    }
}
