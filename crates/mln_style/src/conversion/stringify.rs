// Ports src/mln/style/conversion/stringify.hpp.
//
// gtests-data: ported for test/style/conversion/stringify.test.cpp. The C++
// overload set over a rapidjson writer is the `Stringify` trait here; the
// writer is `rapidjson::Writer<rapidjson::StringBuffer>` (//vendor/rapidjson).
// Nothing in the C++ core calls it except `Properties<...>::stringify`
// (`stringifyLayout`), whose only callers are tests.

use std::collections::HashMap;

use feature::{Identifier, Value};
use mln_style_types::{
    AlignmentType, IconTextFitType, LineCapType, LineJoinType, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType,
    TextJustifyType, TextTransformType, TextWritingModeType, VariableAnchorOffsetCollection, VisibilityType,
};
use mln_util::enum_::Enum;
use mln_util::geometry::FeatureType;
use mln_util::{Color, Padding};
use mln_vec_map::VecMap;
pub use rapidjson::Writer;

use crate::expression::{Formatted, Image, Value as ExpressionValue, ValueConverter};
use crate::filter::Filter;
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;

/// `template <class Writer> void stringify(Writer&, const T&)`.
pub trait Stringify {
    fn stringify(&self, writer: &mut Writer);
}

/// `stringify(writer, t)`.
pub fn stringify<T: Stringify + ?Sized>(writer: &mut Writer, t: &T) {
    t.stringify(writer);
}

/// `NullValue`.
impl Stringify for () {
    fn stringify(&self, writer: &mut Writer) {
        writer.null();
    }
}

impl Stringify for bool {
    fn stringify(&self, writer: &mut Writer) {
        writer.bool_(*self);
    }
}

impl Stringify for u64 {
    fn stringify(&self, writer: &mut Writer) {
        writer.uint64(*self);
    }
}

impl Stringify for i64 {
    fn stringify(&self, writer: &mut Writer) {
        writer.int64(*self);
    }
}

impl Stringify for f64 {
    fn stringify(&self, writer: &mut Writer) {
        writer.double(*self);
    }
}

/// C++ has no `float` overload: a `float` promotes to the `double` one.
impl Stringify for f32 {
    fn stringify(&self, writer: &mut Writer) {
        writer.double(f64::from(*self));
    }
}

impl Stringify for String {
    fn stringify(&self, writer: &mut Writer) {
        writer.string(self);
    }
}

impl Stringify for str {
    fn stringify(&self, writer: &mut Writer) {
        writer.string(self);
    }
}

/// The `std::is_enum_v<T>` overload: `writer.String(Enum<T>::toString(v))`.
/// Rust's coherence rules forbid a blanket impl over `Enum`, so it is
/// implemented for each enum a property can hold.
macro_rules! stringify_enum {
    ($($t:ty),+ $(,)?) => {
        $(impl Stringify for $t {
            fn stringify(&self, writer: &mut Writer) {
                writer.string(Enum::to_string(*self));
            }
        })+
    };
}

stringify_enum!(
    AlignmentType,
    IconTextFitType,
    LineCapType,
    LineJoinType,
    SymbolAnchorType,
    SymbolPlacementType,
    SymbolZOrderType,
    TextJustifyType,
    TextTransformType,
    TextWritingModeType,
    VisibilityType,
);

impl Stringify for Color {
    fn stringify(&self, writer: &mut Writer) {
        writer.string(&Color::stringify(self));
    }
}

impl Stringify for VariableAnchorOffsetCollection {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_array();

        for pair in self.iter() {
            writer.string(Enum::to_string(pair.anchor_type));
            writer.start_array();
            writer.double(f64::from(pair.offset[0]));
            writer.double(f64::from(pair.offset[1]));
            writer.end_array();
        }

        writer.end_array();
    }
}

impl Stringify for [f32; 2] {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_array();
        writer.double(f64::from(self[0]));
        writer.double(f64::from(self[1]));
        writer.end_array();
    }
}

impl Stringify for [f32; 4] {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_array();
        writer.double(f64::from(self[0]));
        writer.double(f64::from(self[1]));
        writer.double(f64::from(self[2]));
        writer.double(f64::from(self[3]));
        writer.end_array();
    }
}

impl Stringify for [f64; 3] {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_array();
        writer.double(self[0]);
        writer.double(self[1]);
        writer.double(self[2]);
        writer.end_array();
    }
}

impl Stringify for Padding {
    fn stringify(&self, writer: &mut Writer) {
        // `stringify(writer, v.toArray())`: top, right, bottom, left.
        [self.top, self.right, self.bottom, self.left].stringify(writer);
    }
}

impl<T: Stringify> Stringify for Vec<T> {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_array();
        for e in self {
            e.stringify(writer);
        }
        writer.end_array();
    }
}

/// `std::unordered_map<std::string, T>`. The C++ iterates in the hash
/// table's order; a `HashMap`'s order differs from libstdc++'s, so the
/// output of maps with more than one entry is not comparable.
impl<T: Stringify> Stringify for HashMap<String, T> {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_object();
        for (key, value) in self {
            writer.key(key);
            value.stringify(writer);
        }
        writer.end_object();
    }
}

/// `mapbox::base::ValueObject` is a `VecMap` in the port (key order).
impl<T: Stringify> Stringify for VecMap<String, T> {
    fn stringify(&self, writer: &mut Writer) {
        writer.start_object();
        for (key, value) in self {
            writer.key(key);
            value.stringify(writer);
        }
        writer.end_object();
    }
}

/// `stringify(writer, const Value&)`: `Value::visit`.
impl Stringify for Value {
    fn stringify(&self, writer: &mut Writer) {
        match self {
            Value::Null => ().stringify(writer),
            Value::Bool(b) => b.stringify(writer),
            Value::Uint(u) => u.stringify(writer),
            Value::Int(i) => i.stringify(writer),
            Value::Double(d) => d.stringify(writer),
            Value::String(s) => s.stringify(writer),
            Value::Array(a) => a.stringify(writer),
            Value::Object(o) => o.stringify(writer),
        }
    }
}

impl Stringify for FeatureType {
    fn stringify(&self, writer: &mut Writer) {
        match self {
            FeatureType::Unknown => writer.string("Unknown"),
            FeatureType::Point => writer.string("Point"),
            FeatureType::LineString => writer.string("LineString"),
            FeatureType::Polygon => writer.string("Polygon"),
        };
    }
}

/// `stringify(writer, const FeatureIdentifier&)`: `FeatureIdentifier::visit`.
impl Stringify for Identifier {
    fn stringify(&self, writer: &mut Writer) {
        match self {
            Identifier::Null => ().stringify(writer),
            Identifier::Uint(u) => u.stringify(writer),
            Identifier::Int(i) => i.stringify(writer),
            Identifier::Double(d) => d.stringify(writer),
            Identifier::String(s) => s.stringify(writer),
        }
    }
}

impl Stringify for Filter {
    fn stringify(&self, writer: &mut Writer) {
        match &self.expression {
            None => {
                writer.null();
            }
            Some(expression) => expression.serialize().stringify(writer),
        }
    }
}

/// Serialization strategy for Formatted objects is to return the constant
/// expression that would generate them.
impl Stringify for Formatted {
    fn stringify(&self, writer: &mut Writer) {
        let value = <Value as ValueConverter>::from_expression_value(&ExpressionValue::Formatted(self.clone()));
        value.unwrap_or(Value::Null).stringify(writer);
    }
}

impl Stringify for Image {
    fn stringify(&self, writer: &mut Writer) {
        let value = <Value as ValueConverter>::from_expression_value(&ExpressionValue::Image(self.clone()));
        value.unwrap_or(Value::Null).stringify(writer);
    }
}

impl<T> Stringify for PropertyExpression<T> {
    fn stringify(&self, writer: &mut Writer) {
        self.get_expression().serialize().stringify(writer);
    }
}

/// `v.evaluate([&](const auto& v_) { stringify(writer, v_); })`.
impl<T: Stringify> Stringify for PropertyValue<T> {
    fn stringify(&self, writer: &mut Writer) {
        match self {
            PropertyValue::Undefined => {
                // `stringify(writer, const Undefined&)`.
                debug_assert!(false, "Should be omitted entirely instead.");
                writer.null();
            }
            PropertyValue::Constant(t) => t.stringify(writer),
            PropertyValue::Expression(e) => e.stringify(writer),
        }
    }
}

/// `template <class Property, class Writer, class T> void stringify(Writer&,
/// const PropertyValue<T>&)`: the property's key and value, unless the value
/// is undefined.
pub fn stringify_property<T: Stringify>(writer: &mut Writer, name: &str, value: &PropertyValue<T>) {
    if !value.is_undefined() {
        writer.key(name);
        value.stringify(writer);
    }
}
