//! `format-section-override`: a property's value that a section of a
//! `formatted` value can override. The style installs it on a symbol layer's
//! `text-color`; it has no style JSON of its own.

use std::sync::Arc;

use feature::Value as MbglValue;
use mln_util::Color;

use crate::expression::{EvalResult, EvaluationContext, Expr, GlobalStateMap, Node, Output, SharedExpr};
use crate::value::Value;

/// The value a property takes where no section overrides it.
#[derive(Clone, PartialEq)]
pub enum OverrideDefault {
    Constant(Color),
    /// The property's expression, and the global state captured for it,
    /// which stands in when an evaluation carries none.
    Expression {
        expression: SharedExpr,
        captured_global_state: Option<Arc<GlobalStateMap>>,
    },
}

#[derive(Clone, PartialEq)]
pub struct FormatSectionOverride {
    default_value: OverrideDefault,
    property_name: String,
}

impl FormatSectionOverride {
    /// An override of the color property `property_name`.
    #[must_use]
    pub fn new_expr(default_value: OverrideDefault, property_name: String) -> Expr {
        Expr::new(Node::FormatSectionOverride(Box::new(FormatSectionOverride { default_value, property_name })))
    }

    #[must_use]
    pub fn default_value(&self) -> &OverrideDefault {
        &self.default_value
    }

    #[must_use]
    pub fn property_name(&self) -> &str {
        &self.property_name
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        if let OverrideDefault::Expression { expression, .. } = &self.default_value {
            visit(expression);
        }
    }

    pub(crate) fn eval_color(&self, context: &EvaluationContext<'_>) -> EvalResult<Color> {
        if let Some(Value::Object(section)) = context.formatted_section
            && let Some(hit) = section.get(&self.property_name)
        {
            return Color::from_value(hit.clone());
        }
        match &self.default_value {
            OverrideDefault::Expression { expression, captured_global_state } => {
                // Binder contexts do not carry global state, so the captured
                // state is used as a fallback. If the caller explicitly
                // provides context state, it takes precedence.
                if context.global_state.is_none()
                    && let Some(state) = captured_global_state
                {
                    let mut context_with_state = *context;
                    context_with_state.global_state = Some(state);
                    return expression.eval_color(&context_with_state);
                }
                expression.eval_color(context)
            }
            OverrideDefault::Constant(color) => Ok(*color),
        }
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String("format-section-override".to_owned())];
        self.each_child(&mut |child| serialized.push(child.serialize()));
        MbglValue::Array(serialized)
    }
}
