use feature::Value;
use mln_style_types::{Position, ProjectionDefinition, Rotation, VariableAnchorOffsetCollection};
use mln_util::{Color, Padding};

use crate::color_ramp_property_value::ColorRampPropertyValue;
use crate::expression::{Formatted, Image};
use crate::property_value::PropertyValue;
use crate::style_property::{Kind, StyleProperty};
use crate::transition_options::TransitionOptions;

/// Makes a `Value` from a style value, and gives the style property kind of a
/// constant (`Color`, `Padding` and `VariableAnchorOffsetCollection`
/// constants are reported as expressions, as in C++).
pub(crate) trait ValueFactory {
    fn make_value(&self) -> Value;

    #[must_use]
    fn constant_kind() -> Kind {
        Kind::Constant
    }
}

impl ValueFactory for TransitionOptions {
    fn make_value(&self) -> Value {
        self.serialize()
    }
}

impl ValueFactory for Color {
    fn make_value(&self) -> Value {
        self.serialize()
    }

    fn constant_kind() -> Kind {
        Kind::Expression
    }
}

impl ValueFactory for Padding {
    fn make_value(&self) -> Value {
        self.serialize()
    }

    fn constant_kind() -> Kind {
        Kind::Expression
    }
}

impl ValueFactory for ProjectionDefinition {
    fn make_value(&self) -> Value {
        self.serialize()
    }
}

impl ValueFactory for VariableAnchorOffsetCollection {
    fn make_value(&self) -> Value {
        self.serialize()
    }

    fn constant_kind() -> Kind {
        Kind::Expression
    }
}

/// Widened to a double.
impl ValueFactory for f32 {
    fn make_value(&self) -> Value {
        Value::Double(f64::from(*self))
    }
}

impl ValueFactory for f64 {
    fn make_value(&self) -> Value {
        Value::Double(*self)
    }
}

impl ValueFactory for bool {
    fn make_value(&self) -> Value {
        Value::Bool(*self)
    }
}

impl ValueFactory for String {
    fn make_value(&self) -> Value {
        Value::String(self.clone())
    }
}

/// The spherical coordinates as an array.
impl ValueFactory for Position {
    fn make_value(&self) -> Value {
        self.get_spherical().make_value()
    }
}

impl ValueFactory for Rotation {
    fn make_value(&self) -> Value {
        Value::Double(self.get_angle())
    }
}

impl ValueFactory for Image {
    fn make_value(&self) -> Value {
        self.to_value()
    }
}

impl ValueFactory for Formatted {
    fn make_value(&self) -> Value {
        self.to_object()
    }
}

impl ValueFactory for ColorRampPropertyValue {
    fn make_value(&self) -> Value {
        self.get_expression().serialize()
    }
}

/// An array of the elements' values.
impl<T: ValueFactory, const N: usize> ValueFactory for [T; N] {
    fn make_value(&self) -> Value {
        Value::Array(self.iter().map(ValueFactory::make_value).collect())
    }
}

impl<T: ValueFactory> ValueFactory for Vec<T> {
    fn make_value(&self) -> Value {
        Value::Array(self.iter().map(ValueFactory::make_value).collect())
    }
}

/// Implements `ValueFactory` for enums, which make their names.
macro_rules! enum_value_factory {
    ($($t:ty),* $(,)?) => {
        $(impl ValueFactory for $t {
            fn make_value(&self) -> Value {
                Value::String(self.as_str().to_owned())
            }
        })*
    };
}

enum_value_factory!(
    mln_style_types::AlignmentType,
    mln_style_types::CirclePitchScaleType,
    mln_style_types::HillshadeIlluminationAnchorType,
    mln_style_types::HillshadeMethodType,
    mln_style_types::IconOverlapType,
    mln_style_types::IconTextFitType,
    mln_style_types::LightAnchorType,
    mln_style_types::LineCapType,
    mln_style_types::LineJoinType,
    mln_style_types::RasterResamplingType,
    mln_style_types::ResamplingType,
    mln_style_types::SymbolAnchorType,
    mln_style_types::SymbolPlacementType,
    mln_style_types::SymbolZOrderType,
    mln_style_types::TextJustifyType,
    mln_style_types::TextTransformType,
    mln_style_types::TextWritingModeType,
    mln_style_types::TranslateAnchorType,
    mln_style_types::VisibilityType,
);

pub(crate) fn make_value<T: ValueFactory>(arg: &T) -> Value {
    arg.make_value()
}

pub(crate) fn make_style_property<T: ValueFactory>(value: &PropertyValue<T>) -> StyleProperty {
    match value {
        PropertyValue::Undefined => StyleProperty::default(),
        PropertyValue::Constant(t) => StyleProperty::new(make_value(t), T::constant_kind()),
        PropertyValue::Expression(fn_) => StyleProperty::new(fn_.get_expression().serialize(), Kind::Expression),
    }
}

#[must_use]
pub(crate) fn make_transition_style_property(value: &TransitionOptions) -> StyleProperty {
    if !value.is_defined() {
        return StyleProperty::default();
    }
    StyleProperty::new(make_value(value), Kind::Transition)
}

#[must_use]
pub(crate) fn make_color_ramp_style_property(value: &ColorRampPropertyValue) -> StyleProperty {
    if value.is_undefined() {
        return StyleProperty::default();
    }
    StyleProperty::new(make_value(value), Kind::Expression)
}
