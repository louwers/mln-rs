// Ports include/mln/style/expression/length.hpp and
// src/mln/style/expression/length.cpp.

use mln_conversion::Convertible;

use crate::expression::{
    EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::utf8_op_helpers::unicode_length_on_validated_utf8;
use crate::value::{Value, type_of};

pub struct Length {
    base: ExpressionBase,
    input: Box<dyn Expression>,
}

impl Length {
    #[must_use]
    pub fn new(input: Box<dyn Expression>) -> Self {
        Length { base: ExpressionBase::new(Kind::Length, Type::Number, input.dependencies()), input }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let length = value.array_length();

        if length != 2 {
            ctx.error(format!("Expected one argument, but found {length} instead."));
            return None;
        }

        let input = ctx.parse(&value.array_member(1), 1, None, None)?;

        let t = input.get_type();
        if !t.is_array() && *t != Type::String && *t != Type::Value {
            ctx.error(format!("Expected argument of type string or array, but found {} instead.", type_::to_string(t)));
            return None;
        }

        Some(Box::new(Length::new(input)))
    }
}

impl Expression for Length {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let value = self.input.evaluate(params)?;
        match &value {
            Value::String(s) => Ok(Value::Number(unicode_length_on_validated_utf8(s) as f64)),
            Value::Array(v) => Ok(Value::Number(v.len() as f64)),
            _ => Err(EvaluationError::new(format!(
                "Expected value to be of type string or array, but found {} instead.",
                type_::to_string(&type_of(&value))
            ))),
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.input);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Length {
            let eq = downcast::<Length>(e).expect("Length");
            return eq.input.equals(&*self.input);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn get_operator(&self) -> String {
        "length".to_owned()
    }
}
