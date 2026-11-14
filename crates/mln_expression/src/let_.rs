// The module is `let_` because `let` is a Rust keyword.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use feature::Value as MbglValue;
use mln_conversion::{Convertible, get_json_type};

use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::parsing_context::{Bindings, ParseResult, ParsingContext};
use crate::value::Value;

pub(crate) struct Let {
    base: ExpressionBase,
    bindings: Bindings,
    result: Box<dyn Expression>,
}

impl Let {
    #[must_use]
    pub(crate) fn new(bindings: Bindings, result: Box<dyn Expression>) -> Self {
        let deps = bindings.values().fold(Dependency::NONE, |a, e| a | e.dependencies())
            | result.dependencies()
            | Dependency::BIND;
        Let { base: ExpressionBase::new(Kind::Let, result.get_type().clone(), deps), bindings, result }
    }

    #[must_use]
    pub(crate) fn get_result(&self) -> &dyn Expression {
        &*self.result
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let length = value.array_length();

        if length < 4 {
            ctx.error(format!("Expected at least 3 arguments, but found {} instead.", length - 1));
            return None;
        }

        let mut bindings = Bindings::new();
        let mut i = 1;
        while i < length - 1 {
            let Some(name) = value.array_member(i).to_string() else {
                ctx.error_at(
                    format!("Expected string, but found {} instead.", get_json_type(&value.array_member(i))),
                    i,
                );
                return None;
            };

            let is_valid_name = name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
            if !is_valid_name {
                ctx.error_at("Variable names must contain only alphanumeric characters or '_'.".to_owned(), 1);
                return None;
            }

            let binding_value = ctx.parse(&value.array_member(i + 1), i + 1, None, None)?;

            // std::map::emplace keeps the first binding of a name.
            bindings.entry(name).or_insert_with(|| Arc::from(binding_value));
            i += 2;
        }

        let result =
            ctx.parse_with_bindings(&value.array_member(length - 1), length - 1, ctx.get_expected(), &bindings)?;

        Some(Box::new(Let::new(bindings, result)))
    }
}

impl Expression for Let {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        self.result.evaluate(params)
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for binding in self.bindings.values() {
            visit(&**binding);
        }
        visit(&*self.result);
    }

    /// The result alone, as `equals` ignores the bindings.
    fn hash_structure(&self, mut state: &mut dyn Hasher) {
        self.get_kind().hash(&mut state);
        self.result.hash_structure(state);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Let {
            let rhs = downcast::<Let>(e).expect("Let");
            return self.result.equals(&*rhs.result);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        self.result.possible_outputs()
    }

    fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator())];
        for (name, value) in &self.bindings {
            serialized.push(MbglValue::String(name.clone()));
            serialized.push(value.serialize());
        }
        serialized.push(self.result.serialize());
        MbglValue::Array(serialized)
    }

    fn get_operator(&self) -> String {
        "let".to_owned()
    }
}

pub(crate) struct Var {
    base: ExpressionBase,
    name: String,
    value: Arc<dyn Expression>,
}

impl Var {
    pub(crate) fn new(name: String, value: Arc<dyn Expression>) -> Self {
        Var {
            base: ExpressionBase::new(Kind::Var, value.get_type().clone(), value.dependencies() | Dependency::VAR),
            name,
            value,
        }
    }

    #[must_use]
    pub(crate) fn get_bound_expression(&self) -> &Arc<dyn Expression> {
        &self.value
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());

        let name = if value.array_length() == 2 { value.array_member(1).to_string() } else { None };
        let Some(name) = name else {
            ctx.error("'var' expression requires exactly one string literal argument.".to_owned());
            return None;
        };

        let Some(binding_value) = ctx.get_binding(&name) else {
            ctx.error_at(
                format!(
                    r#"Unknown variable "{name}". Make sure "{name}" has been bound in an enclosing "let" expression before using it."#
                ),
                1,
            );
            return None;
        };

        Some(Box::new(Var::new(name, binding_value)))
    }
}

impl Expression for Var {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        self.value.evaluate(params)
    }

    fn each_child<'a>(&'a self, _: &mut dyn FnMut(&'a dyn Expression)) {}

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Var {
            let rhs = downcast::<Var>(e).expect("Var");
            return self.value.equals(&*rhs.value);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), MbglValue::String(self.name.clone())])
    }

    fn get_operator(&self) -> String {
        "var".to_owned()
    }
}
