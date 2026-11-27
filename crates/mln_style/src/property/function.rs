//! Legacy functions and `{token}` strings, rewritten as the expressions the
//! style specification defines them by. The parser then types and checks
//! them as it does any other expression.

use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::{Expr, Functions, ParsingContext, Type};
use crate::feature::{Value, ValueObject};

/// The parser's errors for an expression a conversion wrote, without their
/// positions: those are in that expression, not in the style.
pub(crate) fn conversion_errors(ctx: &ParsingContext) -> String {
    ctx.get_errors().into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n")
}

/// `[name, args...]`.
pub(crate) fn op(name: &str, args: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(std::iter::once(Value::from(name)).chain(args).collect())
}

pub(crate) fn get(key: &str) -> Value {
    op("get", [key.into()])
}

/// A copy of `value`.
pub(crate) fn owned(value: &Convertible<'_>) -> Value {
    if value.is_undefined() {
        Value::Null
    } else if value.is_array() {
        Value::Array((0..value.array_length()).map(|i| owned(&value.array_member(i))).collect())
    } else if value.is_object() {
        let mut object = ValueObject::new();
        value.each_member(|key, member| {
            object.insert(key.to_owned(), owned(member));
            None
        });
        Value::Object(object)
    } else {
        value.to_value().unwrap_or(Value::Null)
    }
}

/// An expression that evaluates to `value`.
pub(crate) fn literal(value: Value) -> Value {
    match value {
        Value::Array(_) | Value::Object(_) => op("literal", [value]),
        value => value,
    }
}

pub(crate) fn number(value: &Value) -> Option<f64> {
    match *value {
        Value::Uint(n) => Some(n as f64),
        Value::Int(n) => Some(n as f64),
        Value::Double(n) => Some(n),
        _ => None,
    }
}

/// The largest integer that `match` takes as a label, 2^53 - 1.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Whether `match` takes `labels` as they are: all strings, or all integers.
pub(crate) fn matchable(labels: &[Value]) -> bool {
    labels.iter().all(|label| matches!(label, Value::String(_)))
        || labels.iter().all(|label| number(label).is_some_and(|n| n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER))
}

enum Piece<'a> {
    Text(&'a str),
    Token(&'a str),
}

/// `source` split into text and the names of its `{token}`s.
fn pieces(source: &str) -> Vec<Piece<'_>> {
    let bytes = source.as_bytes();
    let end = bytes.len();
    let mut pieces = Vec::new();
    let mut pos = 0;
    while pos != end {
        let open = bytes[pos..].iter().position(|&c| c == b'{').map_or(end, |o| pos + o);
        if pos != open {
            pieces.push(Piece::Text(&source[pos..open]));
        }
        if open == end {
            break;
        }
        let close = bytes[open + 1..].iter().position(|&c| c == b'{' || c == b'}').map_or(end, |o| open + 1 + o);
        if close != end && bytes[close] == b'}' {
            pieces.push(Piece::Token(&source[open + 1..close]));
            pos = close + 1;
        } else {
            pieces.push(Piece::Text(&source[open..close]));
            pos = close;
        }
    }
    pieces
}

/// Whether `source` contains a `{token}`.
#[must_use]
pub(crate) fn has_tokens(source: &str) -> bool {
    pieces(source).iter().any(|piece| matches!(piece, Piece::Token(_)))
}

/// `source` with each `{token}` replaced by the feature's property of that
/// name.
#[must_use]
pub(crate) fn token_string_expression(source: &str) -> Value {
    let mut inputs: Vec<Value> = pieces(source)
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(text) => text.into(),
            Piece::Token(key) => get(key),
        })
        .collect();
    if inputs.len() == 1 { op("to-string", inputs.pop()) } else { op("concat", inputs) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Interval,
    Exponential,
    Categorical,
}

fn interpolatable(type_: &Type) -> bool {
    match type_ {
        Type::Number
        | Type::Color
        | Type::Padding
        | Type::ProjectionDefinition
        | Type::VariableAnchorOffsetCollection => true,
        // A fixed number of numbers, but not a dash array: its patterns
        // don't blend.
        Type::Array(array) if array.item_type == Type::Number => array.n.is_some(),
        Type::Array(array) => interpolatable(&array.item_type),
        _ => false,
    }
}

fn fail<T>(error: &mut Error, message: &str) -> Option<T> {
    error.message = message.to_owned();
    None
}

/// The function's stops, each its input and the expression of its output.
fn stops(
    function: &Convertible<'_>,
    error: &mut Error,
    output: &dyn Fn(Value) -> Value,
) -> Option<Vec<(Value, Value)>> {
    let Some(stops) = function.object_member("stops") else {
        return fail(error, "function value must specify stops");
    };
    if !stops.is_array() {
        return fail(error, "function stops must be an array");
    }
    if stops.array_length() == 0 {
        return fail(error, "function must have at least one stop");
    }
    (0..stops.array_length())
        .map(|i| {
            let stop = stops.array_member(i);
            if !stop.is_array() {
                return fail(error, "function stop must be an array");
            }
            if stop.array_length() != 2 {
                return fail(error, "function stop must have two elements");
            }
            Some((owned(&stop.array_member(0)), output(owned(&stop.array_member(1)))))
        })
        .collect()
}

/// Stops with numeric inputs, in ascending order, keeping the first of
/// equal inputs.
fn numeric(stops: Vec<(Value, Value)>, error: &mut Error) -> Option<Vec<(f64, Value)>> {
    let mut numeric = Vec::with_capacity(stops.len());
    for (input, output) in stops {
        let Some(input) = number(&input) else {
            return fail(error, "stop domain value must be a number");
        };
        numeric.push((input, output));
    }
    numeric.sort_by(|a, b| a.0.total_cmp(&b.0));
    numeric.dedup_by(|b, a| a.0 == b.0);
    Some(numeric)
}

fn step(input: Value, stops: Vec<(f64, Value)>) -> Value {
    let mut expression = vec!["step".into(), input];
    let base = stops[0].1.clone();
    for (i, (label, output)) in stops.into_iter().enumerate() {
        if i > 0 {
            expression.push(label.into());
        }
        expression.push(output);
    }
    // "step" takes at least one stop besides its base.
    if expression.len() == 3 {
        expression.extend([0.0.into(), base]);
    }
    Value::Array(expression)
}

struct Curve {
    operator: &'static str,
    base: f64,
}

impl Curve {
    fn new(function: &Convertible<'_>, type_: &Type, error: &mut Error) -> Option<Curve> {
        let base = match function.object_member("base") {
            None => 1.0,
            Some(base) => match base.to_double() {
                Some(base) => base,
                None => return fail(error, "function base must be a number"),
            },
        };
        let color_space = function.object_member("colorSpace").and_then(|c| c.to_string());
        let operator = match color_space.as_deref() {
            Some("lab") if *type_ == Type::Color => "interpolate-lab",
            Some("hcl") if *type_ == Type::Color => "interpolate-hcl",
            Some("oklch") if *type_ == Type::Color => "interpolate-oklch",
            Some("oklab") if *type_ == Type::Color => "interpolate-oklab",
            _ => "interpolate",
        };
        Some(Curve { operator, base })
    }

    fn interpolate(&self, input: Value, stops: Vec<(f64, Value)>) -> Value {
        let interpolation = if self.base == 1.0 { op("linear", []) } else { op("exponential", [self.base.into()]) };
        let mut expression = vec![self.operator.into(), interpolation, input];
        for (label, output) in stops {
            expression.extend([label.into(), output]);
        }
        Value::Array(expression)
    }
}

/// `expression` where `property` is a number, else `default`.
fn number_or(property: &str, expression: Value, default: Option<&Value>) -> Value {
    match default {
        None => expression,
        Some(default) => {
            op("case", [op("==", [op("typeof", [get(property)]), "number".into()]), expression, default.clone()])
        }
    }
}

/// The output of the stop whose input `property` equals, else `default`.
fn categorical(
    property: &str,
    stops: Vec<(Value, Value)>,
    default: Option<&Value>,
    error: &mut Error,
) -> Option<Value> {
    let mut branches: Vec<(Value, Value)> = Vec::with_capacity(stops.len());
    for (input, output) in stops {
        let input = match input {
            Value::String(_) | Value::Bool(_) => input,
            _ => match number(&input) {
                Some(n) => n.into(),
                None => return fail(error, "stop domain value must be a number, string, or boolean"),
            },
        };
        if !branches.iter().any(|(label, _)| *label == input) {
            branches.push((input, output));
        }
    }
    // Without a default, the property's default value replaces the error.
    let fallback = default.cloned().unwrap_or_else(|| op("error", ["replaced with default".into()]));
    let labels: Vec<Value> = branches.iter().map(|(label, _)| label.clone()).collect();
    let mut expression = Vec::with_capacity(2 * branches.len() + 2);
    if matchable(&labels) {
        expression.extend(["match".into(), get(property)]);
        for (label, output) in branches {
            expression.extend([label, output]);
        }
    } else {
        expression.push("case".into());
        for (label, output) in branches {
            expression.extend([op("==", [get(property), label]), output]);
        }
    }
    expression.push(fallback);
    Some(Value::Array(expression))
}

/// The feature's `property`, as `type_`.
fn identity(type_: &Type, property: &str, default: Option<Value>) -> Value {
    let get = get(property);
    let Some(default) = default else {
        // The parser asserts or coerces a value to the property's type, but
        // it coerces to a string, which a function asserts.
        return if *type_ == Type::String { op("string", [get]) } else { get };
    };
    match type_ {
        Type::Number => op("number", [get, default]),
        Type::String => op("string", [get, default]),
        Type::Boolean => op("boolean", [get, default]),
        Type::Color => op("to-color", [get, default]),
        Type::Padding => op("to-padding", [get, default]),
        Type::Array(array) => {
            let item = match array.item_type {
                Type::Number => "number",
                Type::String => "string",
                Type::Boolean => "boolean",
                _ => "value",
            };
            let length = array.n.map_or(Value::Null, |n| Value::Uint(n as u64));
            op("array", [item.into(), length, get, default])
        }
        _ => op("coalesce", [get, default]),
    }
}

/// `function`, a legacy function, as the expression it stands for, whose
/// output is `type_`. With `convert_tokens`, the `{token}`s of a zoom
/// function's string outputs read the feature's properties. The expression
/// parses against `functions`.
pub fn parse_function(
    function: &Convertible<'_>,
    type_: &Type,
    convert_tokens: bool,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Expr> {
    parse_converted(&function_expression(function, type_, convert_tokens, error)?, type_, functions, error)
}

/// Parses `expression`, which a conversion wrote, as a property's
/// expression whose output is `type_`, against `functions`.
pub(crate) fn parse_converted(
    expression: &Value,
    type_: &Type,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Expr> {
    let mut ctx = ParsingContext::with_expected(functions, type_.clone());
    let parsed = ctx.parse_layer_property_expression(&Convertible::from(expression));
    if parsed.is_none() {
        error.message = conversion_errors(&ctx);
    }
    parsed
}

fn function_expression(
    function: &Convertible<'_>,
    type_: &Type,
    convert_tokens: bool,
    error: &mut Error,
) -> Option<Value> {
    if !function.is_object() {
        return fail(error, "function must be an object");
    }

    // None for an identity function.
    let kind = match function.object_member("type").map(|t| t.to_string()) {
        None if interpolatable(type_) => Some(Kind::Exponential),
        None => Some(Kind::Interval),
        Some(Some(t)) if t == "interval" => Some(Kind::Interval),
        Some(Some(t)) if t == "exponential" && interpolatable(type_) => Some(Kind::Exponential),
        Some(Some(t)) if t == "categorical" => Some(Kind::Categorical),
        Some(Some(t)) if t == "identity" => None,
        Some(_) => return fail(error, "unsupported function type"),
    };
    let default = function.object_member("default").map(|d| literal(owned(&d)));

    let Some(property) = function.object_member("property") else {
        let output = |output: Value| match output {
            Value::String(s) if convert_tokens && has_tokens(&s) => token_string_expression(&s),
            output => literal(output),
        };
        let stops = numeric(stops(function, error, &output)?, error)?;
        let zoom = op("zoom", []);
        return match kind {
            Some(Kind::Interval) => Some(step(zoom, stops)),
            Some(Kind::Exponential) => Some(Curve::new(function, type_, error)?.interpolate(zoom, stops)),
            Some(Kind::Categorical) | None => fail(error, "unsupported function type"),
        };
    };
    let Some(property) = property.to_string() else {
        return fail(error, "function property must be a string");
    };

    let Some(kind) = kind else {
        return Some(identity(type_, &property, default));
    };

    let stops = stops(function, error, &literal)?;
    let curve = Curve::new(function, type_, error)?;
    let input = op("number", [get(&property)]);
    // The expression of a property function, over `stops`.
    let by_property = |stops: Vec<(Value, Value)>, error: &mut Error| match kind {
        Kind::Interval => Some(number_or(&property, step(input.clone(), numeric(stops, error)?), default.as_ref())),
        Kind::Exponential => {
            Some(number_or(&property, curve.interpolate(input.clone(), numeric(stops, error)?), default.as_ref()))
        }
        Kind::Categorical => categorical(&property, stops, default.as_ref(), error),
    };

    if !matches!(stops[0].0, Value::Object(_)) {
        return by_property(stops, error);
    }

    // A zoom-and-property function: a property function at each zoom.
    let mut zooms: Vec<(f64, Vec<(Value, Value)>)> = Vec::new();
    for (input, output) in stops {
        let Value::Object(input) = input else {
            return fail(error, "stop input must be an object");
        };
        let Some(zoom) = input.get("zoom") else {
            return fail(error, "stop input must specify zoom");
        };
        let Some(zoom) = number(zoom) else {
            return fail(error, "stop domain value must be a number");
        };
        let Some(value) = input.get("value") else {
            return fail(error, "stop input must specify value");
        };
        let i = zooms.partition_point(|(z, _)| *z < zoom);
        if zooms.get(i).is_none_or(|(z, _)| *z != zoom) {
            zooms.insert(i, (zoom, Vec::new()));
        }
        zooms[i].1.push((value.clone(), output));
    }
    let zoom_stops =
        zooms.into_iter().map(|(zoom, stops)| Some((zoom, by_property(stops, error)?))).collect::<Option<Vec<_>>>()?;
    let zoom = op("zoom", []);
    Some(if interpolatable(type_) {
        Curve { operator: curve.operator, base: 1.0 }.interpolate(zoom, zoom_stops)
    } else {
        step(zoom, zoom_stops)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(source: &str) -> String {
        crate::conversion::value_to_json(&token_string_expression(source)).to_string()
    }

    // gtest: StyleConversion.TokenStrings
    #[test]
    fn token_strings() {
        assert!(!has_tokens(""));
        assert!(!has_tokens("{"));
        assert!(!has_tokens("{token"));
        assert!(has_tokens("{token}"));
        assert!(has_tokens("token {token}"));
        assert!(has_tokens("{token} {token}"));

        assert_eq!(tokens("{token}"), r#"["to-string",["get","token"]]"#);
        assert_eq!(tokens("token {token}"), r#"["concat","token ",["get","token"]]"#);
        assert_eq!(tokens("{token} token"), r#"["concat",["get","token"]," token"]"#);
        assert_eq!(tokens("{token} {token}"), r#"["concat",["get","token"]," ",["get","token"]]"#);
        assert_eq!(tokens("{token} {token"), r#"["concat",["get","token"]," ","{token"]"#);
        assert_eq!(tokens("{token {token}"), r#"["concat","{token ",["get","token"]]"#);
    }
}
