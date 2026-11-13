// The module is `in_` because `in` is a Rust keyword.

use mln_conversion::Convertible;

use crate::expression::{
    EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, to_string, type_of};

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

pub struct In {
    base: ExpressionBase,
    needle: Box<dyn Expression>,
    haystack: Box<dyn Expression>,
}

impl In {
    #[must_use]
    pub fn new(needle: Box<dyn Expression>, haystack: Box<dyn Expression>) -> Self {
        debug_assert!(is_comparable_type(needle.get_type()));
        debug_assert!(is_searchable_type(haystack.get_type()));
        In {
            base: ExpressionBase::new(Kind::In, Type::Boolean, needle.dependencies() | haystack.dependencies()),
            needle,
            haystack,
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();
        if length != 3 {
            ctx.error(format!("Expected 2 arguments, but found {} instead.", length - 1));
            return None;
        }

        let needle = ctx.parse(&value.array_member(1), 1, Some(Type::Value), None)?;
        let haystack = ctx.parse(&value.array_member(2), 2, Some(Type::Value), None)?;

        let needle_type = needle.get_type().clone();
        let haystack_type = haystack.get_type().clone();

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
        Some(Box::new(In::new(needle, haystack)))
    }
}

impl Expression for In {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_haystack = self.haystack.evaluate(params)?;
        let evaluated_needle = self.needle.evaluate(params)?;

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
            return Ok(Value::Bool(false));
        }

        if evaluated_haystack_type == Type::String {
            let haystack_string = evaluated_haystack.as_str().unwrap();
            let needle_string = to_string(&evaluated_needle);
            Ok(Value::Bool(haystack_string.contains(needle_string.as_str())))
        } else {
            let haystack_array = evaluated_haystack.as_array().unwrap();
            Ok(Value::Bool(haystack_array.contains(&evaluated_needle)))
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.needle);
        visit(&*self.haystack);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::In {
            let rhs = downcast::<In>(e).expect("In");
            return self.needle.equals(&*rhs.needle) && self.haystack.equals(&*rhs.haystack);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![Some(Value::Bool(true)), Some(Value::Bool(false))]
    }

    fn get_operator(&self) -> String {
        "in".to_owned()
    }
}
