// Ports the style-type parts of include/mln/style/conversion_impl.hpp:
// `ValueFactory<T>`, `makeValue`, `makeStyleProperty`. (`Convertible` itself
// is in mln_conversion.)

use feature::Value;
use mln_style_types::{Position, ProjectionDefinition, Rotation, VariableAnchorOffsetCollection};
use mln_util::enum_::Enum;
use mln_util::{Color, Padding};

use crate::color_ramp_property_value::ColorRampPropertyValue;
use crate::expression::{Formatted, Image};
use crate::property_value::PropertyValue;
use crate::style_property::{Kind, StyleProperty};
use crate::transition_options::TransitionOptions;

/// `ValueFactory<T>::make`, plus the `makeStyleProperty` kind of a constant
/// (`Color`, `Padding` and `VariableAnchorOffsetCollection` constants are
/// reported as expressions, as in C++).
pub trait ValueFactory {
    fn make_value(&self) -> Value;

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

/// `ValueFactory<float>`: `Value(float)` holds a double.
impl ValueFactory for f32 {
    fn make_value(&self) -> Value {
        Value::Double(*self as f64)
    }
}

/// `ValueFactory<double>` (the generic `Value{arg}`).
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

/// `ValueFactory<Position>`: the spherical coordinates as an array.
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

/// `ValueFactory<expression::Formatted>` (include/mln/style/expression/formatted.hpp).
impl ValueFactory for Formatted {
    fn make_value(&self) -> Value {
        self.to_object()
    }
}

/// `ValueFactory<ColorRampPropertyValue>`.
impl ValueFactory for ColorRampPropertyValue {
    fn make_value(&self) -> Value {
        self.get_expression().serialize()
    }
}

/// `ValueFactory<T>` for linear containers.
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

/// `ValueFactory<T, enable_if_t<is_enum_v<T>>>` (the enum's name).
macro_rules! enum_value_factory {
    ($($t:ty),* $(,)?) => {
        $(impl ValueFactory for $t {
            fn make_value(&self) -> Value {
                Value::String(Enum::to_string(*self).to_owned())
            }
        })*
    };
}

enum_value_factory!(
    mln_style_types::AlignmentType,
    mln_style_types::CirclePitchScaleType,
    mln_style_types::HillshadeIlluminationAnchorType,
    mln_style_types::HillshadeMethodType,
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

/// `makeValue(arg)`.
pub fn make_value<T: ValueFactory>(arg: &T) -> Value {
    arg.make_value()
}

/// `makeStyleProperty(const PropertyValue<T>&)`.
pub fn make_style_property<T: ValueFactory>(value: &PropertyValue<T>) -> StyleProperty {
    match value {
        PropertyValue::Undefined => StyleProperty::default(),
        PropertyValue::Constant(t) => StyleProperty::new(make_value(t), T::constant_kind()),
        PropertyValue::Expression(fn_) => StyleProperty::new(fn_.get_expression().serialize(), Kind::Expression),
    }
}

/// `makeStyleProperty(const TransitionOptions&)`.
pub fn make_transition_style_property(value: &TransitionOptions) -> StyleProperty {
    if !value.is_defined() {
        return StyleProperty::default();
    }
    StyleProperty::new(make_value(value), Kind::Transition)
}

/// `makeStyleProperty(const ColorRampPropertyValue&)`.
pub fn make_color_ramp_style_property(value: &ColorRampPropertyValue) -> StyleProperty {
    if value.is_undefined() {
        return StyleProperty::default();
    }
    StyleProperty::new(make_value(value), Kind::Expression)
}
