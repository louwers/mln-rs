// Ports src/mln/renderer/property_evaluator.hpp. It lives in mln_style
// because style/properties.hpp needs it (a cut point of the C++ include
// cycle; see PLAN.md, "Crate map").
//
// C++ evaluators are visitors with three `operator()` overloads that
// `PropertyValue<T>::evaluate` passes to `variant::visit`; `Evaluator<T>` is
// that overload set.

use crate::expression::{EvaluationContext, ValueConverter};
use crate::property_expression::PropertyExpression;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;

/// The `operator()` overload set of a C++ property evaluator.
pub trait Evaluator<T> {
    type ResultType;
    const USE_INTEGER_ZOOM: bool = false;

    fn undefined(&self) -> Self::ResultType;
    fn constant(&self, constant: &T) -> Self::ResultType;
    fn expression(&self, expression: &PropertyExpression<T>) -> Self::ResultType;
}

/// `PropertyEvaluator<T>`.
pub struct PropertyEvaluator<'a, T> {
    parameters: &'a PropertyEvaluationParameters,
    default_value: T,
}

impl<'a, T> PropertyEvaluator<'a, T> {
    pub fn new(parameters: &'a PropertyEvaluationParameters, default_value: T) -> Self {
        PropertyEvaluator { parameters, default_value }
    }
}

impl<T: Clone + ValueConverter> Evaluator<T> for PropertyEvaluator<'_, T> {
    type ResultType = T;

    fn undefined(&self) -> T {
        self.default_value.clone()
    }

    fn constant(&self, constant: &T) -> T {
        constant.clone()
    }

    fn expression(&self, fn_: &PropertyExpression<T>) -> T {
        fn_.evaluate(
            &EvaluationContext::new(self.parameters.z).with_global_state(self.parameters.global_state.as_deref()),
            self.default_value.clone(),
        )
    }
}
