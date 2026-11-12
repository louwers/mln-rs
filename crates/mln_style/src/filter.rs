// Ports include/mln/style/filter.hpp + src/mln/style/filter.cpp.

use std::sync::{Arc, OnceLock};

use feature::Value;
use serde::{Deserialize, Serialize};

use crate::expression::{EvaluationContext, Expression, feature_property_keys, from_expression_value};
use crate::serialization::shared_expression;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Filter {
    /// C++ `std::optional<std::shared_ptr<const expression::Expression>>`
    /// (never an engaged null pointer).
    #[serde(with = "shared_expression::option")]
    pub expression: Option<Arc<dyn Expression>>,
    legacy_filter: Option<Value>,
    /// [`Filter::property_keys`], found on first use.
    #[serde(skip)]
    property_keys: OnceLock<Option<Box<[String]>>>,
}

impl Filter {
    #[must_use]
    pub fn new(expression: Option<Arc<dyn Expression>>, legacy_filter: Option<Value>) -> Self {
        Filter { expression, legacy_filter, property_keys: OnceLock::new() }
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

    /// Port-only: the property keys the filter reads, sorted, if they and
    /// the geometry type are all it reads of a feature
    /// ([`feature_property_keys`]). The filter then selects features with
    /// the same geometry type and the same values under those keys alike.
    /// `None` without an expression.
    pub fn property_keys(&self) -> Option<&[String]> {
        let keys = self.property_keys.get_or_init(|| {
            let expression = self.expression.as_ref()?;
            feature_property_keys(&**expression).map(Vec::into_boxed_slice)
        });
        keys.as_deref()
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
