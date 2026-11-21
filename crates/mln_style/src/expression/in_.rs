// The module is `in_` because `in` is a Rust keyword.

use crate::conversion::Convertible;
use crate::expression::expr::{EvalResult, EvaluationContext, EvaluationError, Expr, Node};
use crate::expression::parsing_context::{ParseResult, ParsingContext};
use crate::expression::type_::{self, Type};
use crate::expression::value::{Value, to_string, type_of};

fn is_comparable_type(t: &Type) -> bool {
    *t == Type::Boolean || *t == Type::String || *t == Type::Number || *t == Type::Null || *t == Type::Value
}

fn is_comparable_runtime_type(t: &Type) -> bool {
    *t == Type::Boolean || *t == Type::String || *t == Type::Number || *t == Type::Null
}

fn is_searchable_type(t: &Type) -> bool {
    *t == Type::String || t.is_array() || *t == Type::Null || *t == Type::Value
}

fn is_searchable_runtime_type(t: &Type) -> bool {
    *t == Type::String || t.is_array() || *t == Type::Null
}

/// `["in", needle, haystack]`.
#[derive(Clone, PartialEq, mln_wire::Wire)]
pub(crate) struct In {
    needle: Expr,
    haystack: Expr,
}

impl In {
    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();
        if length != 3 {
            ctx.error(format!("Expected 2 arguments, but found {} instead.", length - 1));
            return None;
        }

        let needle = ctx.parse(&value.array_member(1), 1, Some(Type::Value), None)?;
        let haystack = ctx.parse(&value.array_member(2), 2, Some(Type::Value), None)?;

        let needle_type = needle.get_type();
        let haystack_type = haystack.get_type();

        if !is_comparable_type(&needle_type) {
            ctx.error(format!(
                "Expected first argument to be of type boolean, string or number, but found {} instead.",
                type_::to_string(&needle_type)
            ));
            return None;
        }

        if !is_searchable_type(&haystack_type) {
            ctx.error(format!(
                "Expected second argument to be of type array or string, but found {} instead.",
                type_::to_string(&haystack_type)
            ));
            return None;
        }
        Some(Expr::new(Node::In(Box::new(In { needle, haystack }))))
    }

    pub(crate) fn eval_bool(&self, params: &EvaluationContext<'_>) -> EvalResult<bool> {
        let evaluated_haystack = self.haystack.evaluate_ref(params)?;
        let evaluated_needle = self.needle.evaluate_ref(params)?;

        let evaluated_needle_type = type_of(&evaluated_needle);
        if !is_comparable_runtime_type(&evaluated_needle_type) {
            return Err(EvaluationError::new(format!(
                "Expected first argument to be of type boolean, string or number, but found {} instead.",
                type_::to_string(&evaluated_needle_type)
            )));
        }

        let evaluated_haystack_type = type_of(&evaluated_haystack);
        if !is_searchable_runtime_type(&evaluated_haystack_type) {
            return Err(EvaluationError::new(format!(
                "Expected second argument to be of type array or string, but found {} instead.",
                type_::to_string(&evaluated_haystack_type)
            )));
        }

        if evaluated_needle_type == Type::Null || evaluated_haystack_type == Type::Null {
            return Ok(false);
        }

        match &*evaluated_haystack {
            Value::String(haystack_string) => Ok(haystack_string.contains(to_string(&evaluated_needle).as_str())),
            Value::Array(haystack_array) => Ok(haystack_array.contains(&evaluated_needle)),
            _ => Ok(false),
        }
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.needle);
        visit(&self.haystack);
    }
}
