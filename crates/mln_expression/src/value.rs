// Ports include/mln/style/expression/value.hpp and
// src/mln/style/expression/value.cpp.

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::value_to_json;
use mln_style_types::{
    AlignmentType, CirclePitchScaleType, HillshadeIlluminationAnchorType, HillshadeMethodType, IconTextFitType,
    LightAnchorType, LineCapType, LineJoinType, Position, ProjectionDefinition, RasterResamplingType, ResamplingType,
    RotateAnchorType, Rotation, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType, TextJustifyType,
    TextTransformType, TextWritingModeType, TranslateAnchorType, VariableAnchorOffsetCollection, VisibilityType,
};
use mln_util::enum_::Enum;
use mln_util::string::to_string_double;
use mln_util::{Color, Padding};
use serde::Serialize;

use crate::collator::Collator;
use crate::formatted::Formatted;
use crate::image::Image;
use crate::type_::{self, Type};

/// The object alternative of `expression::Value`.
// C++ uses std::unordered_map, whose iteration order is libstdc++-hash
// dependent; a VecMap iterates in key order.
pub type ObjectType = mln_vec_map::VecMap<String, Value>;

/// `expression::Value`, same variant order as the C++ variant.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Color(Color),
    VariableAnchorOffsetCollection(VariableAnchorOffsetCollection),
    Collator(Collator),
    Formatted(Formatted),
    Image(Image),
    Padding(Padding),
    ProjectionDefinition(ProjectionDefinition),
    Array(Vec<Value>),
    Object(ObjectType),
}

impl Value {
    /// Javascript's `Number.MAX_SAFE_INTEGER`.
    #[must_use]
    pub const fn max_safe_integer() -> u64 {
        9_007_199_254_740_991
    }

    #[must_use]
    pub const fn is_safe_integer_u64(x: u64) -> bool {
        x <= Self::max_safe_integer()
    }

    #[must_use]
    pub const fn is_safe_integer_i64(x: i64) -> bool {
        // `-x` wraps for INT64_MIN as the C++ compiles.
        (if x > 0 { x } else { x.wrapping_neg() }) as u64 <= Self::max_safe_integer()
    }

    #[must_use]
    pub fn is_safe_integer_f64(x: f64) -> bool {
        // TODO(port): `static_cast<uint64_t>` of |x| >= 2^64 is UB in C++;
        // `as` saturates.
        ((if x > 0.0 { x } else { -x }) as u64) <= Self::max_safe_integer()
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&ObjectType> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_owned())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}
impl From<Color> for Value {
    fn from(v: Color) -> Self {
        Value::Color(v)
    }
}
impl From<Padding> for Value {
    fn from(v: Padding) -> Self {
        Value::Padding(v)
    }
}
impl From<ProjectionDefinition> for Value {
    fn from(v: ProjectionDefinition) -> Self {
        Value::ProjectionDefinition(v)
    }
}
impl From<VariableAnchorOffsetCollection> for Value {
    fn from(v: VariableAnchorOffsetCollection) -> Self {
        Value::VariableAnchorOffsetCollection(v)
    }
}
impl From<Collator> for Value {
    fn from(v: Collator) -> Self {
        Value::Collator(v)
    }
}
impl From<Formatted> for Value {
    fn from(v: Formatted) -> Self {
        Value::Formatted(v)
    }
}
impl From<Image> for Value {
    fn from(v: Image) -> Self {
        Value::Image(v)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}
impl From<ObjectType> for Value {
    fn from(v: ObjectType) -> Self {
        Value::Object(v)
    }
}

/// `typeOf`.
#[must_use]
pub fn type_of(value: &Value) -> Type {
    match value {
        Value::Bool(_) => Type::Boolean,
        Value::Number(_) => Type::Number,
        Value::String(_) => Type::String,
        Value::Color(_) => Type::Color,
        Value::Padding(_) => Type::Padding,
        Value::ProjectionDefinition(_) => Type::ProjectionDefinition,
        Value::VariableAnchorOffsetCollection(_) => Type::VariableAnchorOffsetCollection,
        Value::Collator(_) => Type::Collator,
        Value::Formatted(_) => Type::Formatted,
        Value::Image(_) => Type::Image,
        Value::Null => Type::Null,
        Value::Object(_) => Type::Object,
        Value::Array(arr) => {
            let mut item_type: Option<Type> = None;
            for item in arr {
                let t = type_of(item);
                match &item_type {
                    None => item_type = Some(t),
                    Some(it) if *it == t => {}
                    Some(_) => {
                        item_type = Some(Type::Value);
                        break;
                    }
                }
            }

            Type::array_n(item_type.unwrap_or(Type::Value), arr.len())
        }
    }
}

/// `toString(value)` in GL JS: strings, numbers and booleans as
/// `String(value)`, colors as `rgba(...)`, `null` as `""`, and every other
/// value as [`stringify()`] writes it.
#[must_use]
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Number(n) => to_string_double(*n),
        Value::Color(c) => c.stringify(),                          // avoid quoting
        Value::VariableAnchorOffsetCollection(v) => v.to_string(), // avoid quoting
        Value::Formatted(f) => f.to_string(),
        Value::Image(i) => i.id().to_owned(),
        Value::String(s) => s.clone(), // avoid quoting
        v => stringify(v),
    }
}

/// A JSON number, or `null` for NaN and infinities, as `JSON.stringify`
/// writes them.
fn json_number(n: f64) -> serde_json::Value {
    serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn to_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as Json;
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Number(n) => json_number(*n),
        Value::String(s) => Json::String(s.clone()),
        Value::Color(c) => Json::String(c.stringify()),
        // `toArray()`: top, right, bottom, left.
        Value::Padding(p) => Json::Array([p.top, p.right, p.bottom, p.left].map(|v| json_number(v.into())).into()),
        Value::ProjectionDefinition(p) => value_to_json(&p.serialize()),
        Value::VariableAnchorOffsetCollection(v) => value_to_json(&v.serialize()),
        Value::Collator(_) => {
            // Collators are excluded from constant folding and there's no
            // Literal parser for them so there shouldn't be any way to
            // serialize this value.
            debug_assert!(false, "stringify of a Collator");
            Json::Null
        }
        // Serialization strategy for Formatted objects is to return the
        // constant expression that would generate them.
        Value::Formatted(_) | Value::Image(_) => value_to_json(&to_mbgl_value(value)),
        Value::Array(arr) => Json::Array(arr.iter().map(to_json).collect()),
        Value::Object(obj) => Json::Object(obj.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// Writes numbers as JavaScript does (see [`to_string_double`]).
struct JsNumbers;

impl serde_json::ser::Formatter for JsNumbers {
    fn write_f64<W: ?Sized + std::io::Write>(&mut self, writer: &mut W, value: f64) -> std::io::Result<()> {
        writer.write_all(to_string_double(value).as_bytes())
    }
}

/// `value` as GL JS's `JSON.stringify(value)` writes it: compact, with
/// JavaScript's number formatting, and `null` for NaN and infinities.
#[must_use]
pub fn stringify(value: &Value) -> String {
    let mut json = Vec::new();
    // Writing to a Vec doesn't fail, and serde_json writes UTF-8.
    let _ = to_json(value).serialize(&mut serde_json::Serializer::with_formatter(&mut json, JsNumbers));
    String::from_utf8(json).unwrap_or_default()
}

/// `ValueConverter<mln::Value>::toExpressionValue` (`FromMBGLValue`).
pub fn from_mbgl_value(value: &MbglValue) -> Value {
    match value {
        MbglValue::Array(v) => Value::Array(v.iter().map(from_mbgl_value).collect()),
        MbglValue::Object(v) => Value::Object(v.iter().map(|(k, v)| (k.clone(), from_mbgl_value(v))).collect()),
        MbglValue::String(s) => Value::String(s.clone()),
        MbglValue::Bool(b) => Value::Bool(*b),
        MbglValue::Null => Value::Null,
        MbglValue::Double(v) => Value::Number(*v),
        MbglValue::Uint(v) => Value::Number(*v as f64),
        MbglValue::Int(v) => Value::Number(*v as f64),
    }
}

/// [`from_mbgl_value`] for an owned value, moving its strings, arrays and
/// objects instead of cloning them.
pub fn from_mbgl_value_owned(value: MbglValue) -> Value {
    match value {
        MbglValue::Array(v) => Value::Array(v.into_iter().map(from_mbgl_value_owned).collect()),
        MbglValue::Object(v) => Value::Object(v.into_iter().map(|(k, v)| (k, from_mbgl_value_owned(v))).collect()),
        MbglValue::String(s) => Value::String(s),
        other => from_mbgl_value(&other),
    }
}

/// `ValueConverter<mln::Value>::fromExpressionValue`.
pub fn to_mbgl_value(value: &Value) -> MbglValue {
    match value {
        Value::Color(color) => color.serialize(),
        Value::Padding(padding) => padding.serialize(),
        Value::ProjectionDefinition(projection) => projection.serialize(),
        Value::VariableAnchorOffsetCollection(anchor_offset) => anchor_offset.serialize(),
        Value::Collator(_) => {
            // fromExpressionValue can't be used for Collator values, because
            // they have no meaningful representation as an mln::Value
            debug_assert!(false, "to_mbgl_value of a Collator");
            MbglValue::Null
        }
        Value::Formatted(formatted) => {
            // Serialization strategy for Formatted objects is to return the
            // constant expression that would generate them.
            let mut serialized = vec![MbglValue::String("format".to_owned())];
            for section in &formatted.sections {
                if let Some(image) = &section.image {
                    serialized.push(MbglValue::Array(vec![
                        MbglValue::String("image".to_owned()),
                        MbglValue::String(image.id().to_owned()),
                    ]));
                    continue;
                }

                serialized.push(MbglValue::String(section.text.clone()));
                let mut options = ValueObject::new();

                if let Some(font_scale) = section.font_scale {
                    options.insert("font-scale".to_owned(), MbglValue::Double(font_scale));
                }

                if let Some(font_stack) = &section.font_stack {
                    let font_stack: Vec<MbglValue> = font_stack.iter().map(|f| MbglValue::String(f.clone())).collect();
                    options.insert(
                        "text-font".to_owned(),
                        MbglValue::Array(vec![MbglValue::String("literal".to_owned()), MbglValue::Array(font_stack)]),
                    );
                }

                if let Some(text_color) = &section.text_color {
                    options.insert("text-color".to_owned(), to_mbgl_value(&Value::Color(*text_color)));
                }

                serialized.push(MbglValue::Object(options));
            }
            MbglValue::Array(serialized)
        }
        Value::Image(i) => i.to_value(),
        Value::Array(values) => MbglValue::Array(values.iter().map(to_mbgl_value).collect()),
        Value::Object(values) => MbglValue::Object(values.iter().map(|(k, v)| (k.clone(), to_mbgl_value(v))).collect()),
        Value::Null => MbglValue::Null,
        Value::Bool(b) => MbglValue::Bool(*b),
        Value::Number(d) => MbglValue::Double(*d),
        Value::String(s) => MbglValue::String(s.clone()),
    }
}

/// `ValueConverter<T>`: conversions between style value types and
/// `expression::Value`, plus `valueTypeToExpressionType<T>()`.
pub trait ValueConverter: Sized {
    fn expression_type() -> Type;
    fn to_expression_value(&self) -> Value;
    fn from_expression_value(value: &Value) -> Option<Self>;

    /// `ValueConverter<std::vector<T>>::fromExpressionValue`: an array,
    /// element-wise. C++ specializes it for `std::vector<Color>`
    /// (`hillshade_conversions.cpp`), which also wraps a single color.
    fn vec_from_expression_value(value: &Value) -> Option<Vec<Self>> {
        match value {
            Value::Array(v) => v.iter().map(Self::from_expression_value).collect(),
            _ => None,
        }
    }
}

pub fn to_expression_value<T: ValueConverter>(value: &T) -> Value {
    value.to_expression_value()
}

#[must_use]
pub fn from_expression_value<T: ValueConverter>(value: &Value) -> Option<T> {
    T::from_expression_value(value)
}

#[must_use]
pub fn value_type_to_expression_type<T: ValueConverter>() -> Type {
    T::expression_type()
}

/// `fromExpressionValues<T>`.
#[must_use]
pub fn from_expression_values<T: ValueConverter>(values: &[Option<Value>]) -> Vec<Option<T>> {
    values.iter().map(|value| value.as_ref().and_then(from_expression_value::<T>)).collect()
}

impl ValueConverter for Value {
    fn expression_type() -> Type {
        Type::Value
    }
    fn to_expression_value(&self) -> Value {
        self.clone()
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        Some(value.clone())
    }
}

impl ValueConverter for MbglValue {
    // C++ has no expressionType() for mln::Value.
    fn expression_type() -> Type {
        Type::Value
    }
    fn to_expression_value(&self) -> Value {
        from_mbgl_value(self)
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        Some(to_mbgl_value(value))
    }
}

impl ValueConverter for f32 {
    fn expression_type() -> Type {
        Type::Number
    }
    fn to_expression_value(&self) -> Value {
        Value::Number(f64::from(*self))
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        value.as_number().map(|d| d as f32)
    }
}

macro_rules! simple_value_converter {
    ($t:ty, $variant:ident, $type_:expr) => {
        impl ValueConverter for $t {
            fn expression_type() -> Type {
                $type_
            }
            fn to_expression_value(&self) -> Value {
                Value::$variant(self.clone())
            }
            fn from_expression_value(value: &Value) -> Option<Self> {
                match value {
                    Value::$variant(v) => Some(v.clone()),
                    _ => None,
                }
            }
        }
    };
}

simple_value_converter!(bool, Bool, Type::Boolean);
simple_value_converter!(f64, Number, Type::Number);
simple_value_converter!(String, String, Type::String);
impl ValueConverter for Color {
    fn expression_type() -> Type {
        Type::Color
    }
    fn to_expression_value(&self) -> Value {
        Value::Color(*self)
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        match value {
            Value::Color(v) => Some(*v),
            _ => None,
        }
    }

    /// `ValueConverter<std::vector<Color>>::fromExpressionValue`
    /// (`hillshade_conversions.cpp)`: an array converts element-wise; a single
    /// color is wrapped in a one-element vector.
    fn vec_from_expression_value(value: &Value) -> Option<Vec<Self>> {
        if let Value::Array(values) = value {
            let mut result = Vec::with_capacity(values.len());
            for v in values {
                result.push(Color::from_expression_value(v)?);
            }
            return Some(result);
        }

        // Handle single color - wrap in array
        let color = Color::from_expression_value(value)?;
        Some(vec![color])
    }
}
simple_value_converter!(Padding, Padding, Type::Padding);
simple_value_converter!(ProjectionDefinition, ProjectionDefinition, Type::ProjectionDefinition);
simple_value_converter!(
    VariableAnchorOffsetCollection,
    VariableAnchorOffsetCollection,
    Type::VariableAnchorOffsetCollection
);
simple_value_converter!(Collator, Collator, Type::Collator);
simple_value_converter!(Formatted, Formatted, Type::Formatted);
simple_value_converter!(Image, Image, Type::Image);
simple_value_converter!(ObjectType, Object, Type::Object);

impl<T: ValueConverter> ValueConverter for Vec<T> {
    fn expression_type() -> Type {
        Type::array(T::expression_type())
    }
    fn to_expression_value(&self) -> Value {
        Value::Array(self.iter().map(T::to_expression_value).collect())
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        T::vec_from_expression_value(value)
    }
}

impl<T: ValueConverter, const N: usize> ValueConverter for [T; N] {
    fn expression_type() -> Type {
        Type::array_n(T::expression_type(), N)
    }
    fn to_expression_value(&self) -> Value {
        Value::Array(self.iter().map(T::to_expression_value).collect())
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        match value {
            Value::Array(v) => {
                if v.len() != N {
                    return None;
                }
                let items: Option<Vec<T>> = v.iter().map(T::from_expression_value).collect();
                items.and_then(|items| items.try_into().ok())
            }
            _ => None,
        }
    }
}

impl ValueConverter for Position {
    fn expression_type() -> Type {
        Type::array_n(Type::Number, 3)
    }
    fn to_expression_value(&self) -> Value {
        self.get_spherical().to_expression_value()
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        <[f32; 3]>::from_expression_value(value).map(Position::new)
    }
}

impl ValueConverter for Rotation {
    fn expression_type() -> Type {
        Type::Number
    }
    fn to_expression_value(&self) -> Value {
        (self.get_angle() as f32).to_expression_value()
    }
    fn from_expression_value(value: &Value) -> Option<Self> {
        f32::from_expression_value(value).map(|angle| Rotation::new(f64::from(angle)))
    }
}

macro_rules! enum_value_converter {
    ($($t:ty),* $(,)?) => {
        $(
            impl ValueConverter for $t {
                fn expression_type() -> Type {
                    Type::String
                }
                fn to_expression_value(&self) -> Value {
                    Value::String(Enum::to_string(*self).to_owned())
                }
                fn from_expression_value(value: &Value) -> Option<Self> {
                    match value {
                        Value::String(v) => <$t as Enum>::to_enum(v),
                        _ => None,
                    }
                }
            }
        )*
    };
}

enum_value_converter!(
    AlignmentType,
    CirclePitchScaleType,
    HillshadeIlluminationAnchorType,
    HillshadeMethodType,
    IconTextFitType,
    LightAnchorType,
    LineCapType,
    LineJoinType,
    RasterResamplingType,
    ResamplingType,
    RotateAnchorType,
    SymbolAnchorType,
    SymbolPlacementType,
    SymbolZOrderType,
    TextJustifyType,
    TextTransformType,
    TextWritingModeType,
    TranslateAnchorType,
    VisibilityType,
);

/// `type::toString` re-exported next to the value helpers, as the C++ code
/// calls both unqualified.
#[must_use]
pub fn type_to_string(t: &Type) -> String {
    type_::to_string(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_of_arrays() {
        let v = Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]);
        assert_eq!(type_of(&v), Type::array_n(Type::Number, 2));
        let v = Value::Array(vec![Value::Number(1.0), Value::String("a".into())]);
        assert_eq!(type_of(&v), Type::array_n(Type::Value, 2));
        assert_eq!(type_of(&Value::Array(vec![])), Type::array_n(Type::Value, 0));
    }

    #[test]
    fn safe_integers() {
        assert!(Value::is_safe_integer_f64(9_007_199_254_740_991.0));
        assert!(!Value::is_safe_integer_f64(9_007_199_254_740_992.0));
        assert!(!Value::is_safe_integer_i64(i64::MIN));
    }

    /// `to-string` follows GL JS: `String(n)` for a number, and
    /// `JSON.stringify` for an array or object.
    #[test]
    fn to_string_formats_numbers_as_javascript() {
        let n = Value::Number;
        assert_eq!(to_string(&n(1.0)), "1");
        assert_eq!(to_string(&n(1e21)), "1e+21");
        assert_eq!(to_string(&n(f64::NAN)), "NaN");
        let array = Value::Array(vec![n(1.0), n(0.5), n(1e10), n(f64::INFINITY)]);
        assert_eq!(to_string(&array), "[1,0.5,10000000000,null]");
        let object = Value::Object([("a\n".to_owned(), n(-0.0))].into_iter().collect());
        assert_eq!(to_string(&object), r#"{"a\n":0}"#);
    }
}
