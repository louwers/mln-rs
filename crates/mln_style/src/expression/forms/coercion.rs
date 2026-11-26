use std::borrow::Cow;

use feature::{Value as MbglValue, ValueObject};
use mln_util::string::to_string_double;

use crate::color::Color;
use crate::conversion::Convertible;
use crate::expression::formatted::Formatted;
use crate::expression::image::Image;
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::Type;
use crate::expression::value::{Value, stringify, to_string};
use crate::expression::{EvalResult, EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::padding::Padding;
use crate::types::{
    AnchorOffsetPair, ProjectionDefinition, SymbolAnchorType, VariableAnchorOffsetCollection, projection_type_from_name,
};

type CoerceFunction = fn(&Value) -> EvaluationResult;

fn to_boolean(v: &Value) -> bool {
    match v {
        Value::Number(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Image(i) => i.is_available(),
        _ => true,
    }
}

/// A string as a number: what ECMAScript's `ToNumber` reads gives the same
/// value, except `0x`, `0o` and `0b` literals above `u64::MAX`, and the extras
/// Rust's `str::parse` reads, such as `inf`, are accepted too. Anything else
/// is NaN.
fn string_to_number(s: &str) -> f64 {
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let radix = match s.get(..2) {
        _ if s.is_empty() => return 0.0,
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => return s.parse().unwrap_or(f64::NAN),
    };
    u64::from_str_radix(&s[2..], radix).map_or(f64::NAN, |n| n as f64)
}

/// JS `Number(value)`, as GL JS's `to-number` applies it: null and booleans
/// convert, and a value that gives NaN, or isn't a string, number, boolean or
/// null, fails.
///
/// The style spec defines `to-number` on a string as ECMAScript's `ToNumber`
/// (<https://maplibre.org/maplibre-style-spec/expressions/#to-number>,
/// <https://tc39.es/ecma262/#sec-tonumber-applied-to-the-string-type>). As a
/// consumer of styles rather than a validator, this reads a superset of it.
fn to_number(v: &Value) -> EvalResult<f64> {
    let result = match v {
        Value::Null => Some(0.0),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(f) => Some(*f),
        Value::String(s) => Some(string_to_number(s)),
        _ => None,
    };
    match result.filter(|r| !r.is_nan()) {
        Some(r) => Ok(r),
        None => Err(EvaluationError::new(format!("Could not convert {} to number.", stringify(v)))),
    }
}

fn to_color(color_value: &Value) -> EvalResult<Color> {
    match color_value {
        Value::Color(color) => Ok(*color),
        Value::String(color_string) => match Color::parse(color_string) {
            Some(result) => Ok(result),
            None => Err(EvaluationError::new(format!("Could not parse color from value '{color_string}'"))),
        },
        Value::Array(components) => match components.as_slice() {
            [Value::Number(r), Value::Number(g), Value::Number(b)] => rgba(*r, *g, *b, 1.0),
            [Value::Number(r), Value::Number(g), Value::Number(b), Value::Number(a)] => rgba(*r, *g, *b, *a),
            _ => Err(EvaluationError::new(format!(
                "Invalid rbga value {}: expected an array containing either three or four numeric values.",
                stringify(color_value)
            ))),
        },
        _ => Err(EvaluationError::new(format!("Could not parse color from value '{}'", stringify(color_value)))),
    }
}

/// A padding from 1 to 4 values, in CSS order, or `None` for any other count.
fn padding_from_span(values: &[f32]) -> Option<Padding> {
    match *values {
        [all] => Some(Padding::new(all, all, all, all)),
        [vertical, horizontal] => Some(Padding::new(vertical, horizontal, vertical, horizontal)),
        [top, horizontal, bottom] => Some(Padding::new(top, horizontal, bottom, horizontal)),
        [top, right, bottom, left] => Some(Padding::new(top, right, bottom, left)),
        _ => None,
    }
}

fn to_projection_definition(value: &Value) -> EvaluationResult {
    let error =
        || Err(EvaluationError::new(format!("Could not parse projectionDefinition from value '{}'", stringify(value))));
    match value {
        Value::ProjectionDefinition(projection) => Ok(Value::ProjectionDefinition(*projection)),
        Value::String(name) => match projection_type_from_name(name) {
            Some(type_) => Ok(Value::ProjectionDefinition(ProjectionDefinition::new(type_))),
            None => error(),
        },
        Value::Array(components) => {
            if let [Value::String(from), Value::String(to), Value::Number(transition)] = components.as_slice()
                && let (Some(from), Some(to)) = (projection_type_from_name(from), projection_type_from_name(to))
            {
                return Ok(Value::ProjectionDefinition(ProjectionDefinition::between(from, to, *transition)));
            }
            error()
        }
        _ => error(),
    }
}

fn to_padding(padding_value: &Value) -> EvaluationResult {
    match padding_value {
        Value::Padding(padding) => Ok(Value::Padding(*padding)),
        Value::Array(components) => {
            let components_as_floats: Option<Vec<f32>> =
                components.iter().map(|c| c.as_number().map(|n| n as f32)).collect();
            if let Some(padding) = components_as_floats.as_deref().and_then(padding_from_span) {
                Ok(Value::Padding(padding))
            } else {
                Err(EvaluationError::new(format!(
                    "Invalid padding value {}: expected an array containing from one to four numeric values.",
                    stringify(padding_value)
                )))
            }
        }
        Value::Number(number) => {
            let v = *number as f32;
            Ok(Value::Padding(Padding::new(v, v, v, v)))
        }
        _ => Err(EvaluationError::new(format!("Could not parse padding from value '{}'", stringify(padding_value)))),
    }
}

fn to_variable_anchor_offset(value: &Value) -> EvaluationResult {
    match value {
        Value::VariableAnchorOffsetCollection(anchor_offset) => {
            Ok(Value::VariableAnchorOffsetCollection(anchor_offset.clone()))
        }
        Value::Array(components) => {
            let components_value = || stringify(&Value::Array(components.clone()));
            if components.len() % 2 != 0 {
                return Err(EvaluationError::new(format!(
                    "Invalid variableAnchorOffset value {}: expected an array containing an even number of values.",
                    components_value()
                )));
            }

            let mut anchor_offsets = Vec::new();
            let mut i = 0;
            while i < components.len() {
                let anchor_type_value = &components[i];
                let offset_array = &components[i + 1];

                let (Value::String(anchor_type_string), Value::Array(offset)) = (anchor_type_value, offset_array)
                else {
                    return Err(EvaluationError::new(format!(
                        "Invalid variableAnchorOffset value {}: expected a string value as the first and an array with two elements as the second element of each pair.",
                        components_value()
                    )));
                };
                let [Value::Number(x), Value::Number(y)] = offset.as_slice() else {
                    return Err(EvaluationError::new(format!(
                        "Invalid variableAnchorOffset value {}: expected a string value as the first and an array with two elements as the second element of each pair.",
                        components_value()
                    )));
                };

                let Ok(anchor) = anchor_type_string.parse::<SymbolAnchorType>() else {
                    return Err(EvaluationError::new(format!(
                        "Invalid variableAnchorOffsetCollection value {}: unknown anchor type '{}'",
                        components_value(),
                        anchor_type_string
                    )));
                };

                anchor_offsets.push(AnchorOffsetPair::new(anchor, [*x as f32, *y as f32]));
                i += 2;
            }

            Ok(Value::VariableAnchorOffsetCollection(VariableAnchorOffsetCollection::new(anchor_offsets)))
        }
        _ => Err(EvaluationError::new(format!("Could not parse variableAnchorOffset from '{}'", stringify(value)))),
    }
}

fn to_formatted(formatted_value: &Value) -> Value {
    Value::Formatted(Formatted::from_plain(&to_string(formatted_value)))
}

fn to_image(image_value: &Value) -> Value {
    Value::Image(Image::new(to_string(image_value)))
}

fn to_string_value(value: &Value) -> Value {
    Value::String(to_string(value))
}

fn get_coerce_function(t: &Type) -> CoerceFunction {
    match t {
        Type::Boolean => |v| Ok(Value::Bool(to_boolean(v))),
        Type::Color => |v| to_color(v).map(Value::Color),
        Type::Padding => to_padding,
        Type::ProjectionDefinition => to_projection_definition,
        Type::VariableAnchorOffsetCollection => to_variable_anchor_offset,
        Type::Number => |v| to_number(v).map(Value::Number),
        Type::String => |v| Ok(to_string_value(v)),
        Type::Formatted => |v| Ok(to_formatted(v)),
        Type::Image => |v| Ok(to_image(v)),
        _ => {
            debug_assert!(false, "no coercion to {t}");
            |v| Ok(to_string_value(v))
        }
    }
}

/// `["to-number", ...]` and the other coercions, explicit or inserted by
/// the parser: the first input that converts, or the last one's error.
#[derive(Clone, PartialEq, mln_wire::Wire)]
pub(crate) struct Coercion {
    type_: Type,
    inputs: Box<[Expr]>,
}

impl Coercion {
    pub(crate) fn new_expr(type_: Type, inputs: Vec<Expr>) -> Expr {
        assert!(!inputs.is_empty(), "Coercion without inputs");
        // `isRuntimeConstant` does not consider values coerced to `Type`
        // image to be of `Kind` image, so we don't either (no extra
        // dependency).
        Expr::new(Node::Coercion(Box::new(Coercion { type_, inputs: inputs.into() })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    pub(crate) fn inputs(&self) -> &[Expr] {
        &self.inputs
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let length = value.array_length();

        if length < 2 {
            ctx.error("Expected at least one argument.".to_owned());
            return None;
        }

        let op = value.array_member(0).to_string();
        let type_ = match op.as_deref() {
            Some("to-boolean") => Type::Boolean,
            Some("to-color") => Type::Color,
            Some("to-padding") => Type::Padding,
            Some("to-number") => Type::Number,
            Some("to-string") => Type::String,
            _ => {
                ctx.error(format!("Unknown coercion {}.", op.as_deref().unwrap_or_default()));
                return None;
            }
        };

        if (type_ == Type::Boolean || type_ == Type::String || type_ == Type::Formatted || type_ == Type::Image)
            && length != 2
        {
            ctx.error("Expected one argument.".to_owned());
            return None;
        }

        // Special form for error-coalescing coercion expressions "to-number",
        // "to-color", "to-padding". Since these coercions can fail at
        // runtime, they accept multiple arguments, only evaluating one at a
        // time until one succeeds.
        let mut parsed = Vec::with_capacity(length - 1);
        for i in 1..length {
            let input = ctx.parse(&value.array_member(i), i, Some(Type::Value), None)?;
            parsed.push(input);
        }

        Some(Coercion::new_expr(type_, parsed))
    }

    /// Converts each input's value with `coerce` until one converts.
    fn coerce<T>(&self, params: &EvaluationContext<'_>, coerce: impl Fn(&Value) -> EvalResult<T>) -> EvalResult<T> {
        for (i, input) in self.inputs.iter().enumerate() {
            let value = input.evaluate_ref(params)?;
            let coerced = coerce(&value);
            if coerced.is_ok() || i == self.inputs.len() - 1 {
                return coerced;
            }
        }
        unreachable!("Coercion without inputs")
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        self.coerce(params, get_coerce_function(&self.type_))
    }

    pub(crate) fn eval_num(&self, params: &EvaluationContext<'_>) -> EvalResult<f64> {
        debug_assert_eq!(self.type_, Type::Number);
        self.coerce(params, to_number)
    }

    pub(crate) fn eval_bool(&self, params: &EvaluationContext<'_>) -> EvalResult<bool> {
        debug_assert_eq!(self.type_, Type::Boolean);
        self.coerce(params, |v| Ok(to_boolean(v)))
    }

    pub(crate) fn eval_str<'e>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>> {
        debug_assert_eq!(self.type_, Type::String);
        // The input of a string coercion that is already a string is
        // borrowed, not converted.
        let input = &self.inputs[0];
        if input.get_type() == Type::String {
            return input.eval_str(params);
        }
        self.coerce(params, |v| Ok(Cow::Owned(to_string(v))))
    }

    pub(crate) fn eval_color(&self, params: &EvaluationContext<'_>) -> EvalResult<Color> {
        debug_assert_eq!(self.type_, Type::Color);
        self.coerce(params, to_color)
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        match &self.type_ {
            Type::Formatted => {
                // Since there's no explicit "to-formatted" coercion, the only
                // coercions should be created by string expressions that get
                // implicitly coerced to "formatted".
                MbglValue::Array(vec![
                    MbglValue::String("format".to_owned()),
                    self.inputs[0].serialize(),
                    MbglValue::Object(ValueObject::new()),
                ])
            }
            // Port-only: there is no "to-image" either, and C++ writes
            // `["image", input]`, which parses back to an image expression
            // that asserts a string and checks availability, where the
            // coercion takes any value. The parser makes the coercion
            // implicitly where an image is expected and the input is a string
            // or a value, so the input alone parses back to it. Later inputs
            // never matter, since `to_image` can't fail.
            Type::Image => self.inputs[0].serialize(),
            // There is no coercion operator to these types either. The parser
            // makes the coercion implicitly where one is expected and the
            // input is an array or a value (or a string, for a projection
            // definition), so the input alone parses back to it.
            Type::ProjectionDefinition | Type::VariableAnchorOffsetCollection if self.inputs.len() == 1 => {
                self.inputs[0].serialize()
            }
            _ => {
                let mut serialized = vec![MbglValue::String(self.get_operator().to_owned())];
                serialized.extend(self.inputs.iter().map(Expr::serialize));
                MbglValue::Array(serialized)
            }
        }
    }

    pub(crate) fn get_operator(&self) -> &'static str {
        match &self.type_ {
            Type::Boolean => "to-boolean",
            Type::Color => "to-color",
            Type::Padding => "to-padding",
            Type::ProjectionDefinition => "to-projection-definition",
            Type::Number => "to-number",
            Type::String => "to-string",
            Type::VariableAnchorOffsetCollection => "to-variableanchoroffset",
            _ => {
                debug_assert!(false);
                ""
            }
        }
    }
}

fn stringify_color(r: f64, g: f64, b: f64, a: f64) -> String {
    [r, g, b, a].map(to_string_double).join(", ")
}

pub(crate) fn rgba(r: f64, g: f64, b: f64, a: f64) -> EvalResult<Color> {
    if r < 0.0 || r > 255.0 || g < 0.0 || g > 255.0 || b < 0.0 || b > 255.0 {
        return Err(EvaluationError::new(format!(
            "Invalid rgba value [{}]: 'r', 'g', and 'b' must be between 0 and 255.",
            stringify_color(r, g, b, a)
        )));
    }
    if a < 0.0 || a > 1.0 {
        return Err(EvaluationError::new(format!(
            "Invalid rgba value [{}]: 'a' must be between 0 and 1.",
            stringify_color(r, g, b, a)
        )));
    }
    // C++ `Color(r, g, b, a)` asserts 0..1 per channel (only NaN can get
    // here out of range); the unchecked constructor keeps the release
    // behaviour.
    Ok(Color::new_unchecked((r / 255.0 * a) as f32, (g / 255.0 * a) as f32, (b / 255.0 * a) as f32, a as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::functions::Functions;
    use crate::expression::value::Value;

    /// Strings `ToNumber` reads, with the values JS gives.
    #[test]
    fn strings_convert_as_to_number() {
        let cases = [
            ("12", 12.0),
            (" \t\n\u{a0}\u{feff}\u{3000}-1.5e3\u{2028} ", -1500.0),
            ("", 0.0),
            ("  ", 0.0),
            ("+.5", 0.5),
            ("5.", 5.0),
            ("1e400", f64::INFINITY),
            ("-1e-400", -0.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("0x1F", 31.0),
            ("0o17", 15.0),
            ("0B101", 5.0),
            ("0xFFFFFFFFFFFFFFFF", 2f64.powi(64)),
        ];
        for (s, expected) in cases {
            let n = string_to_number(s);
            assert!(n == expected && n.is_sign_negative() == expected.is_sign_negative(), "{s:?}: {n}");
        }
        for nan in ["12px", "50 mph", "1e", ".", "+", "-0x10", "0x", "0xg", "0b2", "NaN", "1_000"] {
            assert!(string_to_number(nan).is_nan(), "{nan:?}");
        }
    }

    /// The strings `ToNumber` reads that `s.trim().parse::<f64>()` doesn't,
    /// which `string_to_number` handles itself.
    #[test]
    fn strings_to_number_reads_and_str_parse_does_not() {
        let cases = [("", 0.0), ("0x10", 16.0), ("0o17", 15.0), ("0b101", 5.0), ("\u{feff}1", 1.0)];
        for (s, expected) in cases {
            assert_eq!(string_to_number(s), expected, "{s:?}");
            assert!(s.trim().parse::<f64>().is_err(), "{s:?}");
        }
    }

    /// Strings JS rejects but `str::parse` reads, which are accepted.
    #[test]
    fn strings_beyond_to_number_are_accepted() {
        let cases = [("inf", f64::INFINITY), ("-INFINITY", f64::NEG_INFINITY), ("\u{85}1", 1.0)];
        for (s, expected) in cases {
            assert_eq!(string_to_number(s), expected, "{s:?}");
        }
    }

    /// A projection definition from a value serializes to an expression that
    /// parses back to it.
    #[test]
    fn projection_definition_coercion_parses_back() {
        let parse = |json: &serde_json::Value| {
            ParsingContext::with_expected(&Functions::with_builtins(), Type::ProjectionDefinition)
                .parse_expression(&Convertible::from(json), None)
                .unwrap()
        };
        let json = serde_json::json!(["global-state", "projection"]);
        let expression = parse(&json);
        let serialized = crate::conversion::value_to_json(&expression.serialize());
        assert_eq!(serialized, json);
        assert_eq!(parse(&serialized).serialize(), expression.serialize());
    }

    #[test]
    fn values_convert_as_gl_js_to_number() {
        let number = |v: Value| to_number(&v).ok();
        assert_eq!(number(Value::Null), Some(0.0));
        assert_eq!(number(Value::Bool(true)), Some(1.0));
        assert_eq!(number(Value::Bool(false)), Some(0.0));
        assert_eq!(number(Value::String("0x10".into())), Some(16.0));
        assert_eq!(number(Value::String("12px".into())), None);
        assert_eq!(number(Value::String("nan".into())), None);
        assert_eq!(number(Value::Number(f64::NAN)), None);
        assert_eq!(number(Value::Array(vec![Value::Number(1.0)])), None);
    }
}
