//! `["all", ...]` and `["any", ...]`, which [`Expr::eval_bool`] evaluates
//! with short-circuiting.

use mln_conversion::Convertible;

use crate::expression::Expr;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;

fn parse_inputs(value: &Convertible<'_>, ctx: &mut ParsingContext) -> Option<Vec<Expr>> {
    debug_assert!(value.is_array());
    let length = value.array_length();
    let mut parsed_inputs = Vec::with_capacity(length - 1);
    for i in 1..length {
        let parsed = ctx.parse(&value.array_member(i), i, Some(Type::Boolean), None)?;
        parsed_inputs.push(parsed);
    }
    Some(parsed_inputs)
}

pub(crate) fn parse_all(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    Some(Expr::all(parse_inputs(value, ctx)?))
}

pub(crate) fn parse_any(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    Some(Expr::any(parse_inputs(value, ctx)?))
}
