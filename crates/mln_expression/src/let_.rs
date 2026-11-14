// The module is `let_` because `let` is a Rust keyword.

use std::sync::Arc;

use feature::Value as MbglValue;
use mln_conversion::{Convertible, get_json_type};

use crate::expression::{Expr, Node, SharedExpr};
use crate::parsing_context::{Bindings, ParseResult, ParsingContext};

/// `["let", name, value, ..., result]`.
#[derive(Clone, PartialEq)]
pub(crate) struct Let {
    bindings: Bindings,
    result: Expr,
}

impl Let {
    pub(crate) fn result(&self) -> &Expr {
        &self.result
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
            bindings.entry(name).or_insert_with(|| Arc::new(binding_value));
            i += 2;
        }

        let result =
            ctx.parse_with_bindings(&value.array_member(length - 1), length - 1, ctx.get_expected(), &bindings)?;

        Some(Expr::new(Node::Let(Box::new(Let { bindings, result }))))
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        for binding in self.bindings.values() {
            visit(binding);
        }
        visit(&self.result);
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String("let".to_owned())];
        for (name, value) in &self.bindings {
            serialized.push(MbglValue::String(name.clone()));
            serialized.push(value.serialize());
        }
        serialized.push(self.result.serialize());
        MbglValue::Array(serialized)
    }
}

/// `["var", name]`: the expression its enclosing `let` bound to the name,
/// which it evaluates in its place.
#[derive(Clone, PartialEq)]
pub(crate) struct Var {
    name: String,
    value: SharedExpr,
}

impl Var {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn bound_expression(&self) -> &Expr {
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

        Some(Expr::new(Node::Var(Box::new(Var { name, value: binding_value }))))
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        MbglValue::Array(vec![MbglValue::String("var".to_owned()), MbglValue::String(self.name.clone())])
    }
}
