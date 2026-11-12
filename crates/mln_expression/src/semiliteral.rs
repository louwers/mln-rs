// Ports include/mln/style/expression/semiliteral.hpp and
// src/mln/style/expression/semiliteral.cpp.

use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{
    EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, children_equal, collect_dependencies,
    downcast,
};
use crate::literal::Literal;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::Value;

fn array_type(elements: &[Box<dyn Expression>]) -> type_::Array {
    let item_type = if elements.is_empty() { Type::Value } else { elements[0].get_type().clone() };
    let homogeneous = elements.iter().all(|element| *element.get_type() == item_type);
    type_::Array::with_n(if homogeneous { item_type } else { Type::Value }, Some(elements.len()))
}

pub struct Semiliteral {
    base: ExpressionBase,
    elements: Vec<Box<dyn Expression>>,
}

impl Semiliteral {
    #[must_use]
    pub fn new(elements: Vec<Box<dyn Expression>>) -> Self {
        Semiliteral {
            base: ExpressionBase::new(Kind::Semiliteral, array_type(&elements).into(), collect_dependencies(&elements)),
            elements,
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        if value.array_length() != 2 {
            ctx.error(format!(
                "'semiliteral' expression requires exactly one argument, but found {} instead.",
                value.array_length() - 1
            ));
            return None;
        }

        let array = value.array_member(1);
        if !array.is_array() {
            return Literal::parse(value, ctx);
        }

        let length = array.array_length();
        let mut elements = Vec::with_capacity(length);

        for i in 0..length {
            let element = ctx.parse(&array.array_member(i), i, Some(Type::Value), None)?;
            elements.push(element);
        }

        Some(Box::new(Semiliteral::new(elements)))
    }
}

impl Expression for Semiliteral {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, ctx: &EvaluationContext<'_>) -> EvaluationResult {
        let mut values = Vec::with_capacity(self.elements.len());
        for element in &self.elements {
            values.push(element.evaluate(ctx)?);
        }
        Ok(Value::Array(values))
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for element in &self.elements {
            visit(&**element);
        }
    }

    fn equals(&self, other: &dyn Expression) -> bool {
        other.get_kind() == Kind::Semiliteral
            && children_equal(&self.elements, &downcast::<Semiliteral>(other).expect("Semiliteral").elements)
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        let values = self.elements.iter().map(|e| e.serialize()).collect();
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), MbglValue::Array(values)])
    }

    fn get_operator(&self) -> String {
        "semiliteral".to_owned()
    }
}
