use mln_conversion::Convertible;

use crate::expression::{EvalResult, EvaluationContext, Expr, Node, Output};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;

/// `["case", test, output, ..., otherwise]`.
#[derive(Clone, PartialEq)]
pub struct Case {
    type_: Type,
    branches: Box<[(Expr, Expr)]>,
    otherwise: Expr,
}

impl Case {
    /// A `case` of `type_`, with `(test, output)` branches.
    #[must_use]
    pub fn new_expr(type_: Type, branches: Vec<(Expr, Expr)>, otherwise: Expr) -> Expr {
        Expr::new(Node::Case(Box::new(Case { type_, branches: branches.into(), otherwise })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());
        let length = value.array_length();
        if length < 4 {
            ctx.error(format!("Expected at least 3 arguments, but found only {}.", length - 1));
            return None;
        }

        // Expect even-length array: ["case", 2 * (n pairs)..., otherwise]
        if !length.is_multiple_of(2) {
            ctx.error("Expected an odd number of arguments".to_owned());
            return None;
        }

        let mut output_type: Option<Type> = None;
        if let Some(expected) = ctx.get_expected()
            && expected != Type::Value
        {
            output_type = Some(expected);
        }

        let mut branches = Vec::with_capacity((length - 2) / 2);
        let mut i = 1;
        while i + 1 < length {
            let test = ctx.parse(&value.array_member(i), i, Some(Type::Boolean), None)?;
            let output = ctx.parse(&value.array_member(i + 1), i + 1, output_type.clone(), None)?;

            if output_type.is_none() {
                output_type = Some(output.get_type());
            }

            branches.push((test, output));
            i += 2;
        }

        let output_type = output_type.expect("case output type");

        let otherwise = ctx.parse(&value.array_member(length - 1), length - 1, Some(output_type.clone()), None)?;

        Some(Case::new_expr(output_type, branches, otherwise))
    }

    /// The output of the first branch whose test passes, or the fallback's.
    pub(crate) fn eval<'e, T: Output<'e>>(&'e self, params: &EvaluationContext<'_>) -> EvalResult<T> {
        for (test, output) in &self.branches {
            if test.eval_bool(params)? {
                return T::eval(output, params);
            }
        }
        T::eval(&self.otherwise, params)
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        for (test, output) in &self.branches {
            visit(test);
            visit(output);
        }
        visit(&self.otherwise);
    }

    /// Visits each output, the fallback last.
    pub(crate) fn each_output<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        for (_, output) in &self.branches {
            visit(output);
        }
        visit(&self.otherwise);
    }
}
