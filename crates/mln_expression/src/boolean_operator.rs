use mln_conversion::Convertible;

use crate::expression::{
    EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, children_equal, collect_dependencies,
    downcast,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

fn parse_boolean_op(value: &Convertible<'_>, ctx: &mut ParsingContext) -> Option<Vec<Box<dyn Expression>>> {
    debug_assert!(value.is_array());
    let length = value.array_length();

    let mut parsed_inputs = Vec::with_capacity(length - 1);
    for i in 1..length {
        let parsed = ctx.parse(&value.array_member(i), i, Some(Type::Boolean), None)?;
        parsed_inputs.push(parsed);
    }
    Some(parsed_inputs)
}

pub struct Any {
    base: ExpressionBase,
    inputs: Vec<Box<dyn Expression>>,
}

impl Any {
    #[must_use]
    pub fn new(inputs: Vec<Box<dyn Expression>>) -> Self {
        Any { base: ExpressionBase::new(Kind::Any, Type::Boolean, collect_dependencies(&inputs)), inputs }
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        Some(Box::new(Any::new(parse_boolean_op(value, ctx)?)))
    }
}

impl Expression for Any {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        for input in &self.inputs {
            let result = input.evaluate(params)?;
            if result.as_bool().expect("boolean input") {
                return Ok(Value::Bool(true));
            }
        }
        Ok(Value::Bool(false))
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for input in &self.inputs {
            visit(&**input);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Any {
            let rhs = downcast::<Any>(e).expect("Any");
            return children_equal(&self.inputs, &rhs.inputs);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![Some(Value::Bool(true)), Some(Value::Bool(false))]
    }

    fn get_operator(&self) -> String {
        "any".to_owned()
    }
}

pub struct All {
    base: ExpressionBase,
    inputs: Vec<Box<dyn Expression>>,
}

impl All {
    #[must_use]
    pub fn new(inputs: Vec<Box<dyn Expression>>) -> Self {
        All { base: ExpressionBase::new(Kind::All, Type::Boolean, collect_dependencies(&inputs)), inputs }
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        Some(Box::new(All::new(parse_boolean_op(value, ctx)?)))
    }
}

impl Expression for All {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        for input in &self.inputs {
            let result = input.evaluate(params)?;
            if !result.as_bool().expect("boolean input") {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for input in &self.inputs {
            visit(&**input);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::All {
            let rhs = downcast::<All>(e).expect("All");
            return children_equal(&self.inputs, &rhs.inputs);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![Some(Value::Bool(true)), Some(Value::Bool(false))]
    }

    fn get_operator(&self) -> String {
        "all".to_owned()
    }
}
