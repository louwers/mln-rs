//! `["length", input]`, which [`Expr::eval_num`] evaluates with
//! [`evaluate`].

use mln_conversion::Convertible;

use crate::expression::{EvalResult, EvaluationContext, EvaluationError, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::utf8_op_helpers::unicode_length_on_validated_utf8;
use crate::value::{Value, type_of};

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
