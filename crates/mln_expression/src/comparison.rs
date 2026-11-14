use std::cmp::Ordering;

use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::assertion::Assertion;
use crate::collator::Collator;
use crate::expression::{EvalResult, EvaluationContext, EvaluationError, Expr, Node};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::{self, Type};
use crate::value::{Value, type_of};

fn is_comparable_type(op: &str, t: &Type) -> bool {
    if op == "==" || op == "!=" {
        *t == Type::String || *t == Type::Number || *t == Type::Boolean || *t == Type::Null || *t == Type::Value
    } else {
        *t == Type::String || *t == Type::Number || *t == Type::Value
    }
}

/// A comparison operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CmpOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

impl CmpOp {
    fn parse(op: &str) -> CmpOp {
        match op {
            "==" => CmpOp::Eq,
            "!=" => CmpOp::Ne,
            "<" => CmpOp::Lt,
            ">" => CmpOp::Gt,
            "<=" => CmpOp::Le,
            ">=" => CmpOp::Ge,
            _ => unreachable!("unknown comparison {op}"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            CmpOp::Ge => ">=",
        }
    }

    /// Whether `ordering` satisfies the operator; `None` (unordered, a NaN)
    /// satisfies only `!=`.
    fn holds(self, ordering: Option<Ordering>) -> bool {
        match ordering {
            None => self == CmpOp::Ne,
            Some(o) => match self {
                CmpOp::Eq => o.is_eq(),
                CmpOp::Ne => o.is_ne(),
                CmpOp::Lt => o.is_lt(),
                CmpOp::Gt => o.is_gt(),
                CmpOp::Le => o.is_le(),
                CmpOp::Ge => o.is_ge(),
            },
        }
    }
}

/// How a comparison reads its operands, from their static types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Operands {
    Numbers,
    Strings,
    Booleans,
    /// Either side is a value of any type (or null): compared as values,
    /// and ordered only if both turn out strings or both numbers.
    Values,
    /// Strings compared with the collator operand.
    Collated,
}

/// `["==", a, b]` and the other comparisons, with an optional collator.
#[derive(Clone, PartialEq)]
pub(crate) struct Comparison {
    op: CmpOp,
    operands: Operands,
    lhs: Expr,
    rhs: Expr,
    collator: Option<Expr>,
}

impl Comparison {
    pub(crate) fn new_expr(op: &str, lhs: Expr, rhs: Expr, collator: Option<Expr>) -> Expr {
        let (lhs_type, rhs_type) = (lhs.get_type(), rhs.get_type());
        debug_assert!(is_comparable_type(op, &lhs_type) && is_comparable_type(op, &rhs_type));
        let operands = match (&lhs_type, &rhs_type) {
            _ if collator.is_some() => Operands::Collated,
            (Type::Number, Type::Number) => Operands::Numbers,
            (Type::String, Type::String) => Operands::Strings,
            (Type::Boolean, Type::Boolean) => Operands::Booleans,
            _ => Operands::Values,
        };
        Expr::new(Node::Comparison(Box::new(Comparison { op: CmpOp::parse(op), operands, lhs, rhs, collator })))
    }

    /// The operand and the literal of an equality between an expression
    /// and a literal, compared without a collator.
    pub(crate) fn as_equality(&self) -> Option<(&Expr, &Value)> {
        if self.op != CmpOp::Eq || self.collator.is_some() {
            return None;
        }
        match (self.lhs.literal_value(), self.rhs.literal_value()) {
            (None, Some(value)) => Some((&self.lhs, value)),
            (Some(value), None) => Some((&self.rhs, value)),
            _ => None,
        }
    }

    pub(crate) fn get_operator(&self) -> &'static str {
        self.op.as_str()
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(&self.lhs);
        visit(&self.rhs);
        if let Some(collator) = &self.collator {
            visit(collator);
        }
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized =
            vec![MbglValue::String(self.get_operator().to_owned()), self.lhs.serialize(), self.rhs.serialize()];
        if let Some(collator) = &self.collator {
            serialized.push(collator.serialize());
        }
        MbglValue::Array(serialized)
    }

    pub(crate) fn eval_bool(&self, params: &EvaluationContext<'_>) -> EvalResult<bool> {
        match self.operands {
            Operands::Numbers => {
                let (a, b) = (self.lhs.eval_num(params)?, self.rhs.eval_num(params)?);
                Ok(self.op.holds(a.partial_cmp(&b)))
            }
            Operands::Strings => {
                let a = self.lhs.eval_str(params)?;
                let b = self.rhs.eval_str(params)?;
                Ok(self.op.holds(Some(a.cmp(&b))))
            }
            Operands::Booleans => {
                let (a, b) = (self.lhs.eval_bool(params)?, self.rhs.eval_bool(params)?);
                Ok(self.op.holds(Some(a.cmp(&b))))
            }
            Operands::Values => {
                let a = self.lhs.evaluate_ref(params)?;
                let b = self.rhs.evaluate_ref(params)?;
                self.compare_values(&a, &b)
            }
            Operands::Collated => {
                let a = self.lhs.evaluate_ref(params)?;
                let b = self.rhs.evaluate_ref(params)?;
                let (Value::String(a), Value::String(b)) = (&*a, &*b) else {
                    // An equality between a value and a string that turns
                    // out not to be strings compares them as values.
                    return self.compare_values(&a, &b);
                };
                let collator = self.collator.as_ref().expect("collator operand").evaluate_ref(params)?;
                let Value::Collator(collator) = &*collator else { unreachable!("collator operand") };
                Ok(self.op.holds(Some(Collator::compare(collator, a, b))))
            }
        }
    }

    fn compare_values(&self, a: &Value, b: &Value) -> EvalResult<bool> {
        match self.op {
            CmpOp::Eq => Ok(a == b),
            CmpOp::Ne => Ok(a != b),
            _ => match (a, b) {
                (Value::Number(a), Value::Number(b)) => Ok(self.op.holds(a.partial_cmp(b))),
                (Value::String(a), Value::String(b)) => Ok(self.op.holds(Some(a.cmp(b)))),
                _ => Err(EvaluationError::new(format!(
                    r#"Expected arguments for "{}" to be (string, string) or (number, number), but found ({}, {}) instead."#,
                    self.op.as_str(),
                    type_::to_string(&type_of(a)),
                    type_::to_string(&type_of(b))
                ))),
            },
        }
    }
}

pub(crate) fn parse_comparison(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
    let length = value.array_length();

    if length != 3 && length != 4 {
        ctx.error("Expected two or three arguments.".to_owned());
        return None;
    }

    let op = value.array_member(0).to_string().expect("operator name");

    let mut lhs = ctx.parse(&value.array_member(1), 1, Some(Type::Value), None)?;
    let lhs_type = lhs.get_type();
    if !is_comparable_type(&op, &lhs_type) {
        ctx.error_at(
            format!(r#""{}" comparisons are not supported for type '{}'."#, op, type_::to_string(&lhs_type)),
            1,
        );
        return None;
    }

    let mut rhs = ctx.parse(&value.array_member(2), 2, Some(Type::Value), None)?;
    let rhs_type = rhs.get_type();
    if !is_comparable_type(&op, &rhs_type) {
        ctx.error_at(
            format!(r#""{}" comparisons are not supported for type '{}'."#, op, type_::to_string(&rhs_type)),
            2,
        );
        return None;
    }

    if lhs_type != rhs_type && lhs_type != Type::Value && rhs_type != Type::Value {
        ctx.error(format!(
            "Cannot compare types '{}' and '{}'.",
            type_::to_string(&lhs_type),
            type_::to_string(&rhs_type)
        ));
        return None;
    }

    if op != "==" && op != "!=" {
        // typing rules specific to less/greater than operators
        if lhs_type == Type::Value && rhs_type != Type::Value {
            // (value, T)
            lhs = Assertion::new_expr(rhs_type.clone(), vec![lhs]);
        } else if lhs_type != Type::Value && rhs_type == Type::Value {
            // (T, value)
            rhs = Assertion::new_expr(lhs_type.clone(), vec![rhs]);
        }
    }

    if length == 4 {
        if lhs_type != Type::String && rhs_type != Type::String && lhs_type != Type::Value && rhs_type != Type::Value {
            ctx.error("Cannot use collator to compare non-string types.".to_owned());
            return None;
        }
        let collator_parse_result = ctx.parse(&value.array_member(3), 3, Some(Type::Collator), None)?;
        return Some(Comparison::new_expr(&op, lhs, rhs, Some(collator_parse_result)));
    }

    Some(Comparison::new_expr(&op, lhs, rhs, None))
}
