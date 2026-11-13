use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::Dependency;
use crate::property_expression::PropertyExpression;
use crate::renderer::property_evaluator::Evaluator;

/// A style property's value: undefined (the default), a constant or an
/// expression.
#[derive(Debug, Default, Serialize, Deserialize)]
pub enum PropertyValue<T> {
    #[default]
    Undefined,
    Constant(T),
    Expression(PropertyExpression<T>),
}

// Clone and PartialEq are written out rather than derived so that they can be
// `#[inline(never)]`: each generated layer property's setter, `mutable_impl`
// and transition code clone and compare these, and a derive inlined there
// copies the match into every one of them, which costs code size (the browser
// build's wasm most) for no speed that style mutation needs.
impl<T: Clone> Clone for PropertyValue<T> {
    #[inline(never)]
    fn clone(&self) -> Self {
        match self {
            PropertyValue::Undefined => PropertyValue::Undefined,
            PropertyValue::Constant(v) => PropertyValue::Constant(v.clone()),
            PropertyValue::Expression(e) => PropertyValue::Expression(e.clone()),
        }
    }
}

impl<T: PartialEq> PartialEq for PropertyValue<T> {
    #[inline(never)]
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PropertyValue::Undefined, PropertyValue::Undefined) => true,
            (PropertyValue::Constant(a), PropertyValue::Constant(b)) => a == b,
            (PropertyValue::Expression(a), PropertyValue::Expression(b)) => a == b,
            _ => false,
        }
    }
}

impl<T> From<PropertyExpression<T>> for PropertyValue<T> {
    fn from(expression: PropertyExpression<T>) -> Self {
        PropertyValue::Expression(expression)
    }
}

impl<T> PropertyValue<T> {
    pub fn is_undefined(&self) -> bool {
        matches!(self, PropertyValue::Undefined)
    }

    pub fn is_constant(&self) -> bool {
        matches!(self, PropertyValue::Constant(_))
    }

    pub fn is_expression(&self) -> bool {
        matches!(self, PropertyValue::Expression(_))
    }

    pub fn is_data_driven(&self) -> bool {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => false,
            PropertyValue::Expression(fn_) => !fn_.is_feature_constant(),
        }
    }

    pub fn is_zoom_constant(&self) -> bool {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => true,
            PropertyValue::Expression(fn_) => fn_.is_zoom_constant(),
        }
    }

    /// The constant. Panics when the value isn't a constant.
    pub fn as_constant(&self) -> &T {
        match self {
            PropertyValue::Constant(t) => t,
            _ => panic!("PropertyValue::as_constant on a non-constant value"),
        }
    }

    /// The expression. Panics when the value isn't an expression.
    pub fn as_expression(&self) -> &PropertyExpression<T> {
        match self {
            PropertyValue::Expression(e) => e,
            _ => panic!("PropertyValue::as_expression on a non-expression value"),
        }
    }

    pub fn as_expression_mut(&mut self) -> &mut PropertyExpression<T> {
        match self {
            PropertyValue::Expression(e) => e,
            _ => panic!("PropertyValue::as_expression_mut on a non-expression value"),
        }
    }

    /// `evaluate(evaluator)`: visits the alternative with the evaluator.
    pub fn evaluate<E: Evaluator<T>>(&self, evaluator: &E) -> E::ResultType {
        match self {
            PropertyValue::Undefined => evaluator.undefined(),
            PropertyValue::Constant(t) => evaluator.constant(t),
            PropertyValue::Expression(e) => evaluator.expression(e),
        }
    }

    pub fn has_data_driven_property_difference(&self, other: &PropertyValue<T>) -> bool
    where
        T: PartialEq,
    {
        *self != *other && (self.is_data_driven() || other.is_data_driven())
    }

    pub fn get_dependencies(&self) -> Dependency {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => Dependency::NONE,
            PropertyValue::Expression(ex) => ex.get_dependencies(),
        }
    }

    pub fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => None,
            PropertyValue::Expression(ex) => ex.get_global_state_refs(),
        }
    }
}
