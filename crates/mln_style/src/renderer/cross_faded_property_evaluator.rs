// Ports src/mln/renderer/cross_faded_property_evaluator.hpp + .cpp (a cut point of the C++ include cycle).

use mln_util::interpolate::Interpolate;
use serde::{Deserialize, Serialize};

use crate::expression::{EvaluationContext, ValueConverter};
use crate::property_expression::PropertyExpression;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::Evaluator;

/// `Faded<T>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Faded<T> {
    pub from: T,
    pub to: T,
}

/// `Interpolator<Faded<T>> : Uninterpolated`.
impl<T: Clone> Interpolate for Faded<T> {
    fn interpolate(&self, _to: &Self, _t: f64) -> Self {
        self.clone()
    }
}

/// `CrossFadedPropertyEvaluator<T>`.
pub struct CrossFadedPropertyEvaluator<'a, T> {
    parameters: &'a PropertyEvaluationParameters,
    default_value: T,
}

impl<'a, T: Clone> CrossFadedPropertyEvaluator<'a, T> {
    pub fn new(parameters: &'a PropertyEvaluationParameters, default_value: T) -> Self {
        CrossFadedPropertyEvaluator { parameters, default_value }
    }

    fn calculate(&self, min: &T, mid: &T, max: &T) -> Faded<T> {
        let z = self.parameters.z;
        if z > self.parameters.zoom_history.last_integer_zoom {
            Faded { from: min.clone(), to: mid.clone() }
        } else {
            Faded { from: max.clone(), to: mid.clone() }
        }
    }
}

impl<T: Clone + Default + ValueConverter> Evaluator<T> for CrossFadedPropertyEvaluator<'_, T> {
    type ResultType = Faded<T>;

    fn undefined(&self) -> Faded<T> {
        self.calculate(&self.default_value, &self.default_value, &self.default_value)
    }

    fn constant(&self, constant: &T) -> Faded<T> {
        self.calculate(constant, constant, constant)
    }

    fn expression(&self, expression: &PropertyExpression<T>) -> Faded<T> {
        let global_state = self.parameters.global_state.as_deref();
        // C++ evaluates with the default `finalDefaultValue = T()`.
        let z = self.parameters.z;
        self.calculate(
            &expression.evaluate(&EvaluationContext::new(z - 1.0).with_global_state(global_state), T::default()),
            &expression.evaluate(&EvaluationContext::new(z).with_global_state(global_state), T::default()),
            &expression.evaluate(&EvaluationContext::new(z + 1.0).with_global_state(global_state), T::default()),
        )
    }
}
