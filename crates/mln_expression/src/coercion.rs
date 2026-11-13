// Ports include/mln/style/expression/coercion.hpp and
// src/mln/style/expression/coercion.cpp.

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;
use mln_style_types::{
    AnchorOffsetPair, ProjectionDefinition, SymbolAnchorType, VariableAnchorOffsetCollection, projection_type_from_name,
};
use mln_util::Padding;

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, children_equal,
    collect_dependencies, default_serialize, downcast,
};
use crate::formatted::Formatted;
use crate::image::Image;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::util::rgba;
use crate::value::{Value, stringify, to_string};

type CoerceFunction = fn(&Value) -> EvaluationResult;

fn to_boolean(v: &Value) -> Value {
    Value::Bool(match v {
        Value::Number(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Image(i) => i.is_available(),
        _ => true,
    })
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
fn to_number(v: &Value) -> EvaluationResult {
    let result = match v {
        Value::Null => Some(0.0),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(f) => Some(*f),
        Value::String(s) => Some(string_to_number(s)),
        _ => None,
    };
    match result.filter(|r| !r.is_nan()) {
        Some(r) => Ok(Value::Number(r)),
        None => Err(EvaluationError::new(format!("Could not convert {} to number.", stringify(v)))),
    }
}

fn to_color(color_value: &Value) -> EvaluationResult {
    match color_value {
        Value::Color(color) => Ok(Value::Color(*color)),
        Value::String(color_string) => match mln_util::Color::parse(color_string) {
            Some(result) => Ok(Value::Color(result)),
            None => Err(EvaluationError::new(format!("Could not parse color from value '{color_string}'"))),
        },
        Value::Array(components) => {
            let len = components.len();
            let is_numeric = components.iter().all(|item| matches!(item, Value::Number(_)));
            if (len == 3 || len == 4) && is_numeric {
                let n = |i: usize| components[i].as_number().unwrap();
                let c = rgba(n(0), n(1), n(2), if len == 4 { n(3) } else { 1.0 })?;
                Ok(Value::Color(c))
            } else {
                Err(EvaluationError::new(format!(
                    "Invalid rbga value {}: expected an array containing either three or four numeric values.",
                    stringify(color_value)
                )))
            }
        }
        _ => Err(EvaluationError::new(format!("Could not parse color from value '{}'", stringify(color_value)))),
    }
}

/// `Padding(std::span<const float>)`: CSS order, 1 to 4 values.
fn padding_from_span(values: &[f32]) -> Padding {
    match values.len() {
        1 => Padding::new(values[0], values[0], values[0], values[0]),
        2 => Padding::new(values[0], values[1], values[0], values[1]),
        3 => Padding::new(values[0], values[1], values[2], values[1]),
        4 => Padding::new(values[0], values[1], values[2], values[3]),
        _ => panic!("Padding must have between 1 and 4 elements"),
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
            let len = components.len();
            let is_numeric = components.iter().all(|item| matches!(item, Value::Number(_)));
            if (1..=4).contains(&len) && is_numeric {
                let components_as_floats: Vec<f32> = components.iter().map(|c| c.as_number().unwrap() as f32).collect();
                Ok(Value::Padding(padding_from_span(&components_as_floats)))
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
                if offset.len() != 2 {
                    return Err(EvaluationError::new(format!(
                        "Invalid variableAnchorOffset value {}: expected a string value as the first and an array with two elements as the second element of each pair.",
                        components_value()
                    )));
                }

                let Ok(anchor) = anchor_type_string.parse::<SymbolAnchorType>() else {
                    return Err(EvaluationError::new(format!(
                        "Invalid variableAnchorOffsetCollection value {}: unknown anchor type '{}'",
                        components_value(),
                        anchor_type_string
                    )));
                };

                // C++ `get<double>()` throws on a non-number offset.
                // TODO(port): mapbox::util::bad_variant_access propagates out
                // of evaluate() in C++.
                let x = offset[0].as_number().expect("numeric offset") as f32;
                let y = offset[1].as_number().expect("numeric offset") as f32;

                anchor_offsets.push(AnchorOffsetPair::new(anchor, [x, y]));
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
        Type::Boolean => |v| Ok(to_boolean(v)),
        Type::Color => to_color,
        Type::Padding => to_padding,
        Type::ProjectionDefinition => to_projection_definition,
        Type::VariableAnchorOffsetCollection => to_variable_anchor_offset,
        Type::Number => to_number,
        Type::String => |v| Ok(to_string_value(v)),
        Type::Formatted => |v| Ok(to_formatted(v)),
        Type::Image => |v| Ok(to_image(v)),
        _ => {
            debug_assert!(false, "no coercion to {t}");
            |v| Ok(to_string_value(v))
        }
    }
}

pub struct Coercion {
    base: ExpressionBase,
    coerce_single_value: CoerceFunction,
    inputs: Vec<Box<dyn Expression>>,
}

impl Coercion {
    #[must_use]
    pub fn new(type_: Type, inputs: Vec<Box<dyn Expression>>) -> Self {
        assert_ne!(inputs, Vec::<Box<dyn Expression>>::new());
        // `isRuntimeConstant` does not consider values coerced to `Type`
        // image to be of `Kind` image, so we don't either (no extra
        // dependency).
        let coerce_single_value = get_coerce_function(&type_);
        Coercion {
            base: ExpressionBase::new(Kind::Coercion, type_, collect_dependencies(&inputs) | Dependency::NONE),
            coerce_single_value,
            inputs,
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let length = value.array_length();

        if length < 2 {
            ctx.error("Expected at least one argument.".to_owned());
            return None;
        }

        let op = value.array_member(0).to_string().expect("operator name");
        let type_ = match op.as_str() {
            "to-boolean" => Type::Boolean,
            "to-color" => Type::Color,
            "to-padding" => Type::Padding,
            "to-projection-definition" => Type::ProjectionDefinition,
            "to-number" => Type::Number,
            "to-string" => Type::String,
            "to-variableanchoroffset" => Type::VariableAnchorOffsetCollection,
            _ => unreachable!("unknown coercion {op}"),
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

        Some(Box::new(Coercion::new(type_, parsed)))
    }
}

impl Expression for Coercion {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        for (i, input) in self.inputs.iter().enumerate() {
            let value = input.evaluate(params)?;
            let coerced = (self.coerce_single_value)(&value);
            if coerced.is_ok() || i == self.inputs.len() - 1 {
                return coerced;
            }
        }

        unreachable!("Coercion without inputs")
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for input in &self.inputs {
            visit(&**input);
        }
    }

    fn serialize(&self) -> MbglValue {
        // Port-only: the conversion of a token string without tokens (a
        // legacy function's stop, say) makes `["to-string", "text"]`, which
        // the parser folds into the literal it coerces to itself. Writing the
        // literal makes the serialized form the one it parses back to.
        if let [input] = self.inputs.as_slice()
            && input.get_kind() == Kind::Literal
            && input.get_type() == self.get_type()
        {
            return input.serialize();
        }
        match self.get_type() {
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
            // Port-only: C++ writes "to-variableanchoroffset", which no
            // parser knows. The coercion is made implicitly, where a
            // collection is expected and the input is an array or a value, so
            // the input alone parses back to it.
            Type::VariableAnchorOffsetCollection if self.inputs.len() == 1 => self.inputs[0].serialize(),
            _ => default_serialize(self),
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Coercion {
            let rhs = downcast::<Coercion>(e).expect("Coercion");
            return self.get_type() == rhs.get_type() && children_equal(&self.inputs, &rhs.inputs);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for input in &self.inputs {
            result.extend(input.possible_outputs());
        }
        result
    }

    fn get_operator(&self) -> String {
        match self.get_type() {
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
        .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn values_convert_as_gl_js_to_number() {
        let number = |v: Value| to_number(&v).ok().and_then(|n| n.as_number());
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
