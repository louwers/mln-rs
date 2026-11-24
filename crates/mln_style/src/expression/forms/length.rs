//! `["length", input]`, which [`Expr::eval_num`] evaluates with
//! [`evaluate`].

use crate::conversion::Convertible;
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::{self, Type};
use crate::expression::value::{Value, type_of};
use crate::expression::{EvalResult, EvaluationContext, EvaluationError, Expr, Node};

pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    let length = value.array_length();

    if length != 2 {
        ctx.error(format!("Expected one argument, but found {length} instead."));
        return None;
    }

    let input = ctx.parse(&value.array_member(1), 1, None, None)?;

    let t = &input.get_type();
    if !t.is_array() && *t != Type::String && *t != Type::Value {
        ctx.error(format!("Expected argument of type string or array, but found {} instead.", type_::to_string(t)));
        return None;
    }

    Some(Expr::new(Node::Length(Box::new(input))))
}

/// The length of `input`'s string or array.
pub(crate) fn evaluate(input: &Expr, params: &EvaluationContext<'_>) -> EvalResult<f64> {
    let value = input.evaluate_ref(params)?;
    match &*value {
        Value::String(s) => Ok(unicode_length_on_validated_utf8(s) as f64),
        Value::Array(v) => Ok(v.len() as f64),
        _ => Err(EvaluationError::new(format!(
            "Expected value to be of type string or array, but found {} instead.",
            type_::to_string(&type_of(&value))
        ))),
    }
}

fn is_continuation_byte(b: u8) -> bool {
    (b & 0xC0) == 0x80
}

/// Get the number of unicode characters of an utf-8 string.
/// Precondition: the string must be valid utf-8 (otherwise, the result may be
/// incorrect).
#[must_use]
pub(crate) fn unicode_length_on_validated_utf8(s: &str) -> usize {
    s.bytes().filter(|b| !is_continuation_byte(*b)).count()
}

/// Get the byte offset of a unicode character offset in a utf-8 string: the
/// byte offset of the (start of the) unicode character with the requested
/// offset, or `s.len()` if `char_offset` is larger than the number of
/// unicode characters or otherwise invalid.
#[must_use]
pub(crate) fn get_unicode_character_offset_on_validated_utf8(s: &str, char_offset: usize) -> usize {
    let mut cur_char = 0;
    for (i, b) in s.bytes().enumerate() {
        if !is_continuation_byte(b) {
            if cur_char == char_offset {
                return i;
            }
            cur_char += 1;
        }
    }
    s.len()
}
