// Ports include/mln/style/color_ramp_property_value.hpp.

use std::sync::Arc;

use mln_expression::value::from_expression_value;
use mln_util::Color;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, EvaluationContext, Expression, GlobalStateMap, collect_global_state_refs};
use crate::properties::PropertyValueLike;
use crate::renderer::property_evaluator::Evaluator;
use crate::serialization::shared_expression;

/// Special-case implementation of (a subset of) the `PropertyValue<T>`
/// interface used for building the paint property traits classes for
/// HeatmapColor, ColorReliefColor, and LineGradient.
///
/// Generic paint-property evaluation uses a placeholder Color, while the
/// render layers evaluate the expression separately to build their
/// color-ramp textures. Referenced global-state keys are retained so those
/// textures are rebuilt only when one of those keys changes.
///
/// Port-only: serde writes the expression; the global-state keys are
/// collected from it again.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(from = "ColorRampParts", into = "ColorRampParts")]
pub struct ColorRampPropertyValue {
    value: Option<Arc<dyn Expression>>,
    global_state_refs: Option<Arc<VecSet<String>>>,
}

impl PartialEq for ColorRampPropertyValue {
    fn eq(&self, other: &Self) -> bool {
        (self.is_undefined() && other.is_undefined())
            || match (&self.value, &other.value) {
                (Some(lhs), Some(rhs)) => lhs.equals(rhs.as_ref()),
                _ => false,
            }
    }
}

#[derive(Serialize, Deserialize)]
struct ColorRampParts(#[serde(with = "shared_expression::option")] Option<Arc<dyn Expression>>);

impl From<ColorRampParts> for ColorRampPropertyValue {
    fn from(parts: ColorRampParts) -> Self {
        ColorRampPropertyValue::new(parts.0)
    }
}

impl From<ColorRampPropertyValue> for ColorRampParts {
    fn from(value: ColorRampPropertyValue) -> Self {
        ColorRampParts(value.value)
    }
}

impl ColorRampPropertyValue {
    /// `ColorRampPropertyValue(std::shared_ptr<expression::Expression>)`.
    pub fn new(value: Option<Arc<dyn Expression>>) -> Self {
        let global_state_refs = match &value {
            Some(expression) if expression.has(Dependency::GLOBAL_STATE) => {
                let mut refs = VecSet::new();
                collect_global_state_refs(expression.as_ref(), &mut refs);
                Some(Arc::new(refs))
            }
            _ => None,
        };
        ColorRampPropertyValue { value, global_state_refs }
    }

    pub fn is_undefined(&self) -> bool {
        self.value.is_none()
    }

    /// `evaluate(double rampEvaluationParameter, const GlobalStateMap* globalState)`.
    pub fn evaluate_ramp(&self, ramp_evaluation_parameter: f64, global_state: Option<&GlobalStateMap>) -> Color {
        let value = self.value.as_ref().expect("ColorRampPropertyValue::evaluate on an undefined value");
        let result = value.evaluate(
            &EvaluationContext::with_color_ramp(None, None, Some(ramp_evaluation_parameter))
                .with_global_state(global_state),
        );
        let Ok(result) = result else {
            return Color::default();
        };
        from_expression_value::<Color>(&result).unwrap_or_default()
    }

    pub fn is_data_driven(&self) -> bool {
        false
    }

    pub fn has_data_driven_property_difference(&self, _other: &ColorRampPropertyValue) -> bool {
        false
    }

    /// `getExpression()`; C++ requires a defined value.
    pub fn get_expression(&self) -> &dyn Expression {
        self.value.as_deref().expect("ColorRampPropertyValue::get_expression on an undefined value")
    }

    pub fn get_dependencies(&self) -> Dependency {
        match &self.value {
            Some(value) => value.dependencies(),
            None => Dependency::NONE,
        }
    }

    pub fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        self.global_state_refs.as_deref()
    }
}

impl PropertyValueLike for ColorRampPropertyValue {
    type Type = Color;

    /// The batch-evaluation no-op: C++ returns `Color{}` whatever the
    /// evaluator. `ColorRampProperty` evaluates with
    /// `PropertyEvaluator<Color>(parameters, ColorRampProperty::defaultValue())`,
    /// whose `undefined()` is that same `Color{}`.
    fn evaluate<E: Evaluator<Color>>(&self, evaluator: &E) -> E::ResultType {
        evaluator.undefined()
    }

    fn is_undefined(&self) -> bool {
        ColorRampPropertyValue::is_undefined(self)
    }

    fn is_data_driven(&self) -> bool {
        false
    }

    fn get_dependencies(&self) -> Dependency {
        ColorRampPropertyValue::get_dependencies(self)
    }

    fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        ColorRampPropertyValue::get_global_state_refs(self)
    }

    fn has_data_driven_property_difference(&self, _other: &Self) -> bool {
        false
    }
}
