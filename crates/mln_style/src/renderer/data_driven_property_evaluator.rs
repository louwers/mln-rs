use crate::expression::{Dependency, EvaluationContext, ValueConverter};
use crate::property_expression::PropertyExpression;
use crate::renderer::cross_faded_property_evaluator::Faded;
use crate::renderer::possibly_evaluated_property_value::{
    PossiblyEvaluatedFadedPropertyValue, PossiblyEvaluatedPropertyValue,
};
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::Evaluator;

pub struct DataDrivenPropertyEvaluator<'a, T, const USE_INTEGER_ZOOM: bool = false> {
    parameters: &'a PropertyEvaluationParameters,
    default_value: T,
}

impl<'a, T, const USE_INTEGER_ZOOM: bool> DataDrivenPropertyEvaluator<'a, T, USE_INTEGER_ZOOM> {
    pub fn new(parameters: &'a PropertyEvaluationParameters, default_value: T) -> Self {
        DataDrivenPropertyEvaluator { parameters, default_value }
    }
}

impl<T: Clone + ValueConverter, const USE_INTEGER_ZOOM: bool> Evaluator<T>
    for DataDrivenPropertyEvaluator<'_, T, USE_INTEGER_ZOOM>
{
    type ResultType = PossiblyEvaluatedPropertyValue<T>;
    const USE_INTEGER_ZOOM: bool = USE_INTEGER_ZOOM;

    fn undefined(&self) -> Self::ResultType {
        PossiblyEvaluatedPropertyValue::Constant(self.default_value.clone())
    }

    fn constant(&self, constant: &T) -> Self::ResultType {
        PossiblyEvaluatedPropertyValue::Constant(constant.clone())
    }

    fn expression(&self, expression: &PropertyExpression<T>) -> Self::ResultType {
        // Expressions that depend on the feature or on runtime-loaded images
        // cannot be evaluated yet and are kept for later per-feature
        // evaluation. Global-state-only expressions can be evaluated
        // immediately with the state carried by the evaluation parameters.
        let needs_feature =
            !expression.is_feature_constant() || expression.get_dependencies().intersects(Dependency::IMAGE);
        let global_state = self.parameters.global_state.as_deref();
        if USE_INTEGER_ZOOM {
            if needs_feature {
                let mut return_expression = expression.clone();
                return_expression.set_use_integer_zoom(true);
                return_expression.capture_global_state(self.parameters.global_state.clone());
                return PossiblyEvaluatedPropertyValue::Expression(return_expression);
            }
            PossiblyEvaluatedPropertyValue::Constant(expression.evaluate(
                &EvaluationContext::new(self.parameters.z.floor()).with_global_state(global_state),
                self.default_value.clone(),
            ))
        } else {
            if needs_feature {
                let mut return_expression = expression.clone();
                return_expression.capture_global_state(self.parameters.global_state.clone());
                return PossiblyEvaluatedPropertyValue::Expression(return_expression);
            }
            PossiblyEvaluatedPropertyValue::Constant(expression.evaluate(
                &EvaluationContext::new(self.parameters.z).with_global_state(global_state),
                self.default_value.clone(),
            ))
        }
    }
}

/// The data-driven evaluator of cross-faded values.
pub struct DataDrivenFadedPropertyEvaluator<'a, T> {
    parameters: &'a PropertyEvaluationParameters,
    default_value: T,
}

impl<'a, T: Clone> DataDrivenFadedPropertyEvaluator<'a, T> {
    pub fn new(parameters: &'a PropertyEvaluationParameters, default_value: T) -> Self {
        DataDrivenFadedPropertyEvaluator { parameters, default_value }
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

impl<T: Clone + Default + ValueConverter> Evaluator<T> for DataDrivenFadedPropertyEvaluator<'_, T> {
    type ResultType = PossiblyEvaluatedFadedPropertyValue<T>;

    fn undefined(&self) -> Self::ResultType {
        PossiblyEvaluatedFadedPropertyValue::Constant(self.calculate(
            &self.default_value,
            &self.default_value,
            &self.default_value,
        ))
    }

    fn constant(&self, constant: &T) -> Self::ResultType {
        PossiblyEvaluatedFadedPropertyValue::Constant(self.calculate(constant, constant, constant))
    }

    fn expression(&self, expression: &PropertyExpression<T>) -> Self::ResultType {
        if !expression.is_feature_constant() || !expression.is_runtime_constant() {
            let mut return_expression = expression.clone();
            return_expression.capture_global_state(self.parameters.global_state.clone());
            PossiblyEvaluatedFadedPropertyValue::Expression(return_expression)
        } else {
            // Quirk kept: C++ calls `expression.evaluate(std::floor(z))`, the
            // overload without the parameters' global state.
            let evaluated = expression.evaluate_zoom(self.parameters.z.floor());
            PossiblyEvaluatedFadedPropertyValue::Constant(self.calculate(&evaluated, &evaluated, &evaluated))
        }
    }
}
