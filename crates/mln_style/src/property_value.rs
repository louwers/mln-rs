use std::fmt::Debug;
use std::sync::Arc;

use mln_conversion::{Convertible, Error};
use mln_vec_map::VecSet;

use crate::conversion::{Converter, convert_property_value};
use crate::conversion_impl::{ValueFactory, make_style_property};
use crate::evaluated::{Evaluated, Evaluation};
use crate::expression::{Dependency, Functions, Image, ValueConverter, global_state_refs_intersect};
use crate::property_expression::PropertyExpression;
use crate::property_table::PropertyInfo;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::transition_parameters::GlobalStateChange;
use crate::style_property::StyleProperty;

/// A style property's value: undefined (the default), a constant or an
/// expression.
#[derive(Debug, Default, mln_wire::Wire)]
pub enum PropertyValue<T> {
    #[default]
    Undefined,
    Constant(T),
    Expression(PropertyExpression<T>),
}

// Clone and PartialEq are written out rather than derived so that they can be
// `#[inline(never)]`: a layer's impl, its property sets and the transitions
// clone and compare one per property, and a derive inlined there copies the
// match into every one of them, which costs code size (the browser build's
// wasm most) for no speed that style mutation needs.
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
    pub fn is_constant(&self) -> bool {
        matches!(self, PropertyValue::Constant(_))
    }

    pub fn is_expression(&self) -> bool {
        matches!(self, PropertyValue::Expression(_))
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
}

/// A style value a property holds: a [`PropertyValue`], or a
/// [`ColorRampPropertyValue`](crate::color_ramp_property_value::ColorRampPropertyValue).
pub trait StyleValue: Clone + Default + Debug + PartialEq + 'static {
    /// What the value evaluates to.
    type Type: Clone + Debug + Default + ValueConverter;

    /// Converts a style JSON value for the property `info`, parsing
    /// expressions against `functions`.
    fn convert(value: &Convertible<'_>, info: &PropertyInfo, functions: &Arc<Functions>) -> Result<Self, Error>;

    fn style_property(&self) -> StyleProperty;

    /// The value for a frame, with `default` for an undefined value.
    fn evaluate(
        &self,
        evaluation: Evaluation<Self::Type>,
        parameters: &PropertyEvaluationParameters,
        default: Self::Type,
    ) -> Evaluated<Self::Type>;

    fn is_undefined(&self) -> bool;

    /// Whether the value may differ by feature.
    fn is_data_driven(&self) -> bool;

    fn get_dependencies(&self) -> Dependency;

    fn get_global_state_refs(&self) -> Option<&VecSet<String>>;

    /// Whether the values differ and either may differ by feature.
    fn has_data_driven_property_difference(&self, other: &Self) -> bool {
        (self.is_data_driven() || other.is_data_driven()) && self != other
    }

    /// Whether both values are constants, and differ.
    fn constant_differs(&self, _other: &Self) -> bool {
        false
    }

    /// Whether a change of the keys in `changed_keys` transitions the value
    /// rather than snapping it.
    fn transitions_on_global_state_change(&self, _changed_keys: Option<&VecSet<String>>) -> bool {
        false
    }

    /// Keeps the value reading the global state from before `change`, if it
    /// transitions on it.
    fn retain_prior_global_state(&mut self, _change: &GlobalStateChange) {}
}

impl<T> StyleValue for PropertyValue<T>
where
    T: Converter + ValueConverter + ValueFactory + Clone + Debug + Default + PartialEq + 'static,
{
    type Type = T;

    fn convert(value: &Convertible<'_>, info: &PropertyInfo, functions: &Arc<Functions>) -> Result<Self, Error> {
        let mut error = Error::default();
        convert_property_value::<T>(value, &mut error, info.data_driven, info.tokens, functions).ok_or(error)
    }

    fn style_property(&self) -> StyleProperty {
        make_style_property(self)
    }

    fn evaluate(
        &self,
        evaluation: Evaluation<T>,
        parameters: &PropertyEvaluationParameters,
        default: T,
    ) -> Evaluated<T> {
        evaluation(self, parameters, default)
    }

    fn is_undefined(&self) -> bool {
        matches!(self, PropertyValue::Undefined)
    }

    fn is_data_driven(&self) -> bool {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => false,
            PropertyValue::Expression(fn_) => !fn_.is_feature_constant(),
        }
    }

    fn get_dependencies(&self) -> Dependency {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => Dependency::NONE,
            PropertyValue::Expression(ex) => ex.get_dependencies(),
        }
    }

    fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        match self {
            PropertyValue::Undefined | PropertyValue::Constant(_) => None,
            PropertyValue::Expression(ex) => ex.get_global_state_refs(),
        }
    }

    fn constant_differs(&self, other: &Self) -> bool {
        match (self, other) {
            (PropertyValue::Constant(a), PropertyValue::Constant(b)) => a != b,
            _ => false,
        }
    }

    /// A value that reads a changed key transitions unless it differs by
    /// feature or is a pattern image: images are never interpolated, so a
    /// transition would only delay the image the tiles are laid out with.
    fn transitions_on_global_state_change(&self, changed_keys: Option<&VecSet<String>>) -> bool {
        std::any::TypeId::of::<T>() != std::any::TypeId::of::<Image>()
            && !self.is_data_driven()
            && self.get_dependencies().intersects(Dependency::GLOBAL_STATE)
            && global_state_refs_intersect(self.get_global_state_refs(), changed_keys)
    }

    fn retain_prior_global_state(&mut self, change: &GlobalStateChange) {
        if self.is_expression() && self.transitions_on_global_state_change(Some(&change.changed_keys)) {
            self.as_expression_mut().pin_global_state(Arc::clone(&change.prior_state));
        }
    }
}
