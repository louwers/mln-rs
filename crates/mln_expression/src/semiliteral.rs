use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{EvaluationContext, EvaluationResult, Expr, Node};
use crate::literal::Literal;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::Value;

/// `["semiliteral", [...]]`: an array whose elements are expressions.
#[derive(Clone, PartialEq)]
pub(crate) struct Semiliteral {
    elements: Box<[Expr]>,
}

impl Semiliteral {
    #[cfg(test)]
    pub(crate) fn new_expr(elements: Vec<Expr>) -> Expr {
        Expr::new(Node::Semiliteral(Box::new(Semiliteral { elements: elements.into() })))
    }

    pub(crate) fn elements(&self) -> &[Expr] {
        &self.elements
    }

    /// An array of the elements' count, of their type when they share one.
    pub(crate) fn get_type(&self) -> Type {
        let item_type = self.elements.first().map_or(Type::Value, Expr::get_type);
        let homogeneous = self.elements.iter().all(|element| element.get_type() == item_type);
        type_::Array::with_n(if homogeneous { item_type } else { Type::Value }, Some(self.elements.len())).into()
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
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

        Some(Expr::new(Node::Semiliteral(Box::new(Semiliteral { elements: elements.into() }))))
    }

    pub(crate) fn evaluate(&self, ctx: &EvaluationContext<'_>) -> EvaluationResult {
        let mut values = Vec::with_capacity(self.elements.len());
        for element in &self.elements {
            values.push(element.evaluate(ctx)?);
        }
        Ok(Value::Array(values))
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let values = self.elements.iter().map(Expr::serialize).collect();
        MbglValue::Array(vec![MbglValue::String("semiliteral".to_owned()), MbglValue::Array(values)])
    }
}
