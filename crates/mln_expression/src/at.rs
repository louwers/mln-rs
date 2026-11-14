use mln_conversion::Convertible;
use mln_util::string::to_string_double;

use crate::expression::{EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;

/// `["at", index, array]`.
#[derive(Clone, PartialEq)]
pub(crate) struct At {
    index: Expr,
    input: Expr,
}

impl At {
    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());
        let length = value.array_length();
        if length != 3 {
            ctx.error(format!("Expected 2 arguments, but found {} instead.", length - 1));
            return None;
        }

        let index = ctx.parse(&value.array_member(1), 1, Some(Type::Number), None);
        let input_type = Type::array(ctx.get_expected().unwrap_or(Type::Value));
        let input = ctx.parse(&value.array_member(2), 2, Some(input_type), None);

        let (Some(index), Some(input)) = (index, input) else {
            return None;
        };
        debug_assert!(input.get_type().is_array());
        Some(Expr::new(Node::At(Box::new(At { index, input }))))
    }

    /// The array's item type.
    pub(crate) fn get_type(&self) -> Type {
        self.input.get_type().as_array().expect("array input").item_type.clone()
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.index);
        visit(&self.input);
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_index = self.index.eval_num(params);
        let evaluated_input = self.input.evaluate_ref(params);
        let i = evaluated_index?;
        let evaluated_input = evaluated_input?;
        let input_array = evaluated_input.as_array().expect("array input");

        if i < 0.0 {
            return Err(EvaluationError::new(format!("Array index out of bounds: {} < 0.", to_string_double(i))));
        }

        if i >= input_array.len() as f64 {
            return Err(EvaluationError::new(format!(
                "Array index out of bounds: {} > {}.",
                to_string_double(i),
                (input_array.len() as u64).wrapping_sub(1)
            )));
        }
        if i != i.floor() {
            return Err(EvaluationError::new(format!(
                "Array index must be an integer, but found {} instead.",
                to_string_double(i)
            )));
        }
        Ok(input_array[i as usize].clone())
    }
}
