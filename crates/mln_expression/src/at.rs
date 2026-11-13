use mln_conversion::Convertible;
use mln_util::string::to_string_double;

use crate::expression::{
    EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

pub struct At {
    base: ExpressionBase,
    index: Box<dyn Expression>,
    input: Box<dyn Expression>,
}

impl At {
    #[must_use]
    pub fn new(index: Box<dyn Expression>, input: Box<dyn Expression>) -> Self {
        let item_type = input.get_type().as_array().expect("array input").item_type.clone();
        At { base: ExpressionBase::new(Kind::At, item_type, index.dependencies() | input.dependencies()), index, input }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
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

        Some(Box::new(At::new(index, input)))
    }
}

impl Expression for At {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let evaluated_index = self.index.evaluate(params);
        let evaluated_input = self.input.evaluate(params);
        let evaluated_index = evaluated_index?;
        let evaluated_input = evaluated_input?;

        let i = evaluated_index.as_number().expect("numeric index");
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

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.index);
        visit(&*self.input);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::At {
            let rhs = downcast::<At>(e).expect("At");
            return self.index.equals(&*rhs.index) && self.input.equals(&*rhs.input);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn get_operator(&self) -> String {
        "at".to_owned()
    }
}
