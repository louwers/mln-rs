use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{ObjectType, Value, from_mbgl_value, to_mbgl_value, type_of};

/// A constant value, typed as the value is, or as the array type it was
/// parsed as.
#[derive(Clone, PartialEq)]
pub(crate) struct Literal {
    type_: Type,
    value: Value,
}

fn check_number(n: f64) -> Value {
    if n > f64::MAX { Value::Number(f64::INFINITY) } else { Value::Number(n) }
}

#[expect(
    clippy::only_used_in_recursion,
    reason = "`ctx` is only passed on to the recursive calls, as the parsers of other expressions take it"
)]
pub(crate) fn parse_value(value: &Convertible<'_>, ctx: &mut ParsingContext) -> Option<Value> {
    if value.is_undefined() {
        return Some(Value::Null);
    }
    if value.is_object() {
        let mut result = ObjectType::new();
        let mut error = false;
        value.each_member(|k, v| {
            if !error {
                if let Some(member_value) = parse_value(v, ctx) {
                    // unordered_map::emplace keeps the first value of a key.
                    result.entry(k.to_owned()).or_insert(member_value);
                } else {
                    error = true;
                }
            }
            None
        });
        return if error { None } else { Some(Value::Object(result)) };
    }

    if value.is_array() {
        let mut result = Vec::new();
        let length = value.array_length();
        for i in 0..length {
            let item = parse_value(&value.array_member(i), ctx)?;
            result.push(item);
        }
        return Some(Value::Array(result));
    }

    // since value represents a JSON value, if it's not undefined, object, or
    // array, it must be convertible to mln::Value
    let v = value.to_value().expect("to_value of a JSON scalar");

    match v {
        MbglValue::Uint(n) => Some(check_number(n as f64)),
        MbglValue::Int(n) => Some(check_number(n as f64)),
        MbglValue::Double(n) => Some(check_number(n)),
        v => Some(from_mbgl_value(&v)),
    }
}

impl Literal {
    #[must_use]
    pub(crate) fn new(value: Value) -> Self {
        Literal { type_: type_of(&value), value }
    }

    #[must_use]
    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    #[must_use]
    pub(crate) fn get_value(&self) -> &Value {
        &self.value
    }

    /// An array literal typed as `type_`.
    pub(crate) fn array(type_: type_::Array, items: Vec<Value>) -> Expr {
        Expr::new(Node::Literal(Box::new(Literal { type_: type_.into(), value: Value::Array(items) })))
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        if value.is_object() {
            ctx.error(r#"Bare objects invalid. Use ["literal", {...}] instead."#.to_owned());
            None
        } else if value.is_array() {
            if value.array_length() != 2 {
                ctx.error(format!(
                    "'literal' expression requires exactly one argument, but found {} instead.",
                    value.array_length() as u64 - 1
                ));
                return None;
            }
            let parsed_value = parse_value(&value.array_member(1), ctx)?;

            if let (Some(Type::Array(expected)), Value::Array(items)) = (ctx.get_expected(), &parsed_value)
                && let Type::Array(t) = type_of(&parsed_value)
                && t.n == Some(0)
                && (expected.n.is_none() || expected.n == Some(0))
            {
                return Some(Literal::array(*expected, items.clone()));
            }

            Some(Expr::literal(parsed_value))
        } else {
            let parsed_value = parse_value(value, ctx).expect("primitive literal");
            Some(Expr::literal(parsed_value))
        }
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        if let Value::Image(image) = &self.value {
            return MbglValue::String(image.id().to_owned());
        }
        if self.type_.is_array() || self.type_ == Type::Object || self.type_ == Type::VariableAnchorOffsetCollection {
            MbglValue::Array(vec![MbglValue::String("literal".to_owned()), to_mbgl_value(&self.value)])
        } else {
            to_mbgl_value(&self.value)
        }
    }
}
