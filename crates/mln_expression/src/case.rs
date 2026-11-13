use mln_conversion::Convertible;

use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::Value;

pub(crate) type Branch = (Box<dyn Expression>, Box<dyn Expression>);

pub struct Case {
    base: ExpressionBase,
    branches: Vec<Branch>,
    otherwise: Box<dyn Expression>,
}

impl Case {
    #[must_use]
    pub fn new(type_: Type, branches: Vec<Branch>, otherwise: Box<dyn Expression>) -> Self {
        let deps = branches.iter().fold(Dependency::NONE, |a, b| a | b.0.dependencies() | b.1.dependencies())
            | otherwise.dependencies();
        Case { base: ExpressionBase::new(Kind::Case, type_, deps), branches, otherwise }
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
                output_type = Some(output.get_type().clone());
            }

            branches.push((test, output));
            i += 2;
        }

        let output_type = output_type.expect("case output type");

        let otherwise = ctx.parse(&value.array_member(length - 1), length - 1, Some(output_type.clone()), None)?;

        Some(Box::new(Case::new(output_type, branches, otherwise)))
    }
}

impl Expression for Case {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        for branch in &self.branches {
            let evaluated_test = branch.0.evaluate(params)?;
            if evaluated_test.as_bool().expect("boolean test") {
                return branch.1.evaluate(params);
            }
        }

        self.otherwise.evaluate(params)
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for branch in &self.branches {
            visit(&*branch.0);
            visit(&*branch.1);
        }
        visit(&*self.otherwise);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Case {
            let rhs = downcast::<Case>(e).expect("Case");
            return self.otherwise.equals(&*rhs.otherwise)
                && self.branches.len() == rhs.branches.len()
                && self.branches.iter().zip(&rhs.branches).all(|(l, r)| l.0.equals(&*r.0) && l.1.equals(&*r.1));
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for branch in &self.branches {
            result.extend(branch.1.possible_outputs());
        }
        result.extend(self.otherwise.possible_outputs());
        result
    }

    fn get_operator(&self) -> String {
        "case".to_owned()
    }
}
