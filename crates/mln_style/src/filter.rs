// Ports include/mln/style/filter.hpp + src/mln/style/filter.cpp.

use std::sync::Arc;

use feature::Value;
use serde::{Deserialize, Serialize};

use crate::expression::{EvaluationContext, Expression, from_expression_value};
use crate::serialization::shared_expression;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Filter {
    /// C++ `std::optional<std::shared_ptr<const expression::Expression>>`
    /// (never an engaged null pointer).
    #[serde(with = "shared_expression::option")]
    pub expression: Option<Arc<dyn Expression>>,
    legacy_filter: Option<Value>,
}

impl Filter {
    pub fn new(expression: Option<Arc<dyn Expression>>, legacy_filter: Option<Value>) -> Self {
        Filter { expression, legacy_filter }
    }

    /// `operator bool()`.
    pub fn is_set(&self) -> bool {
        self.expression.is_some() || self.legacy_filter.is_some()
    }

    /// `operator()(const expression::EvaluationContext&)`.
    pub fn evaluate(&self, context: &EvaluationContext<'_>) -> bool {
        let Some(expression) = &self.expression else {
            return true;
        };

        match expression.evaluate(context) {
            Ok(result) => from_expression_value::<bool>(&result).unwrap_or(false),
            Err(_) => false,
        }
    }

    pub fn serialize(&self) -> Value {
        if let Some(legacy_filter) = &self.legacy_filter {
            legacy_filter.clone()
        } else if let Some(expression) = &self.expression {
            expression.serialize()
        } else {
            Value::Null
        }
    }
}

impl PartialEq for Filter {
    /// Quirk kept: two set filters compare their `shared_ptr`s, i.e. by
    /// identity, not by expression equality.
    fn eq(&self, other: &Self) -> bool {
        match (&self.expression, &other.expression) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}
