use std::hash::Hasher;
use std::sync::{Arc, OnceLock};

use feature::Value;
use serde::{Deserialize, Serialize};

use crate::expression::{EvaluationContext, Expr, Value as ExpressionValue, feature_property_keys};
use crate::serialization::shared_expression;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Filter {
    #[serde(with = "shared_expression::option")]
    pub expression: Option<Arc<Expr>>,
    legacy_filter: Option<Value>,
    /// [`Filter::property_keys`], found on first use.
    #[serde(skip)]
    property_keys: OnceLock<Option<Box<[String]>>>,
    /// [`Filter::expression_hash`], found on first use.
    #[serde(skip)]
    expression_hash: OnceLock<u64>,
    /// The hash of [`Filter::equality`]'s operand, found on first use.
    #[serde(skip)]
    operand_hash: OnceLock<Option<u64>>,
}

/// A filter that holds where an operand equals a literal
/// ([`Filter::equality`]).
pub struct Equality<'a> {
    pub operand: &'a Expr,
    pub value: &'a ExpressionValue,
    /// A hash of the operand's structure, alike for equal operands.
    pub operand_hash: u64,
}

impl Filter {
    #[must_use]
    pub fn new(expression: Option<Arc<Expr>>, legacy_filter: Option<Value>) -> Self {
        Filter {
            expression,
            legacy_filter,
            property_keys: OnceLock::new(),
            expression_hash: OnceLock::new(),
            operand_hash: OnceLock::new(),
        }
    }

    pub(crate) fn is_set(&self) -> bool {
        self.expression.is_some() || self.legacy_filter.is_some()
    }

    pub fn evaluate(&self, context: &EvaluationContext<'_>) -> bool {
        let Some(expression) = &self.expression else {
            return true;
        };

        expression.eval_bool(context).unwrap_or(false)
    }

    /// Port-only: the property keys the filter reads, sorted, if they and
    /// the geometry type are all it reads of a feature
    /// ([`feature_property_keys`]). The filter then selects features with
    /// the same geometry type and the same values under those keys alike.
    /// `None` without an expression.
    pub fn property_keys(&self) -> Option<&[String]> {
        let keys = self.property_keys.get_or_init(|| {
            let expression = self.expression.as_ref()?;
            feature_property_keys(expression).map(Vec::into_boxed_slice)
        });
        keys.as_deref()
    }

    /// Port-only: a hash of the expression's structure
    /// ([`Expr::hash_structure`]), so that filters whose expressions
    /// are equal hash alike, whether or not they share them.
    pub fn expression_hash(&self) -> u64 {
        *self.expression_hash.get_or_init(|| {
            let mut state = std::hash::DefaultHasher::new();
            if let Some(expression) = &self.expression {
                expression.hash_structure(&mut state);
            }
            state.finish()
        })
    }

    /// Port-only: the operand and literal of a filter `["==", operand,
    /// literal]`, which selects the features whose operand evaluates to a
    /// value equal to the literal. Filters over one source layer that
    /// compare the same operand can then share its value for each feature.
    pub fn equality(&self) -> Option<Equality<'_>> {
        let (operand, value) = self.expression.as_ref()?.as_equality()?;
        let operand_hash = (*self.operand_hash.get_or_init(|| {
            let mut state = std::hash::DefaultHasher::new();
            operand.hash_structure(&mut state);
            Some(state.finish())
        }))?;
        Some(Equality { operand, value, operand_hash })
    }

    pub(crate) fn serialize(&self) -> Value {
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
