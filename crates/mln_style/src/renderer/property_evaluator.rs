// Lives in mln_style because the property system needs it.
//
// An evaluator handles each case of a `PropertyValue<T>` (undefined, a
// constant or an expression); `PropertyValue::evaluate` picks the case.

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
