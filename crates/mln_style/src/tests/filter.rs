use feature::{Identifier, Value};
use mln_conversion::Convertible;
use mln_util::geometry::FeatureType;

use super::util::{StubGeometryTileFeature, parse_json, property_map};
use crate::conversion::{Converter, Error};
use crate::expression::EvaluationContext;
use crate::filter::Filter;

fn convert_json(json: &str, error: &mut Error) -> Option<Filter> {
    let value = parse_json(json);
    <Filter as Converter>::convert(&Convertible::from(&value), error)
}

fn filter_full(json: &str, properties: &[(&str, Value)], id: Identifier, type_: FeatureType, zoom: f32) -> bool {
    let mut error = Error::default();
    let filter = convert_json(json, &mut error);
    assert!(filter.is_some(), "{json}: {}", error.message);
    assert_eq!(error.message, "", "{json}");

    let feature = StubGeometryTileFeature::with_id_type(id, type_, property_map(properties));
    let context = EvaluationContext::with_zoom_feature(zoom, Some(&feature));

    filter.unwrap().evaluate(&context)
}

fn filter(json: &str, properties: &[(&str, Value)]) -> bool {
    filter_full(json, properties, Identifier::Null, FeatureType::Point, 0.0)
}

fn filter_id(json: &str, properties: &[(&str, Value)], id: Identifier) -> bool {
    filter_full(json, properties, id, FeatureType::Point, 0.0)
}

fn filter_type(json: &str, type_: FeatureType) -> bool {
    filter_full(json, &[], Identifier::Null, type_, 0.0)
}

fn invalid_filter(json: &str) {
    let mut error = Error::default();
    let filter = convert_json(json, &mut error);
    assert!(filter.is_none(), "{json}");
    assert_ne!(error.message, "", "{json}");
}

fn stringify_filter(json: &str) -> String {
    let mut error = Error::default();
    let filter = convert_json(json, &mut error);
    assert!(filter.is_some());
    assert_eq!(error.message, "");

    let value = filter.unwrap().serialize();
    mln_expression::value::stringify(&mln_expression::value::from_mbgl_value(&value))
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

// gtest: Filter.EqualsNull
#[test]
fn equals_null() {
    let f = r#"["==", "foo", null]"#;
    assert!(filter(f, &[("foo", Value::Null)]));

    assert!(!filter(f, &[("foo", Value::Int(0))]));
    assert!(!filter(f, &[("foo", Value::Int(1))]));
    assert!(!filter(f, &[("foo", s("0"))]));
    assert!(!filter(f, &[("foo", Value::Bool(true))]));
    assert!(!filter(f, &[("foo", Value::Bool(false))]));
    assert!(!filter(f, &[]));
}

// gtest: Filter.EqualsString
#[test]
fn equals_string() {
    let f = r#"["==", "foo", "bar"]"#;
    assert!(filter(f, &[("foo", s("bar"))]));
    assert!(!filter(f, &[("foo", s("baz"))]));
}

// gtest: Filter.EqualsNumber
#[test]
fn equals_number() {
    let f = r#"["==", "foo", 0]"#;
    assert!(filter(f, &[("foo", Value::Int(0))]));
    assert!(filter(f, &[("foo", Value::Uint(0))]));
    assert!(filter(f, &[("foo", Value::Double(0.0))]));
    assert!(!filter(f, &[("foo", Value::Int(1))]));
    assert!(!filter(f, &[("foo", Value::Uint(1))]));
    assert!(!filter(f, &[("foo", Value::Double(1.0))]));
    assert!(!filter(f, &[("foo", s("0"))]));
    assert!(!filter(f, &[("foo", Value::Bool(false))]));
    assert!(!filter(f, &[("foo", Value::Bool(true))]));
    assert!(!filter(f, &[("foo", Value::Null)]));
    assert!(!filter(f, &[]));
}

// gtest: Filter.EqualsType
#[test]
fn equals_type() {
    let f = r#"["==", "$type", "LineString"]"#;
    assert!(!filter_type(f, FeatureType::Point));
    assert!(filter_type(f, FeatureType::LineString));
    assert!(!filter_type(f, FeatureType::Point));

    invalid_filter(r#"["==", "$type"]"#);
    invalid_filter(r#"["==", "$type", null]"#);
    invalid_filter(r#"["==", "$type", "foo", 1]"#);
    invalid_filter(r#"["==", "$type", "foo", "Point"]"#);
}

// gtest: Filter.InType
#[test]
fn in_type() {
    let f = r#"["in", "$type", "LineString", "Polygon"]"#;
    assert!(!filter_type(f, FeatureType::Point));
    assert!(filter_type(f, FeatureType::LineString));
    assert!(filter_type(f, FeatureType::Polygon));
}

// gtest: Filter.InID
#[test]
fn in_id() {
    let f = r#"["in", "$id", "123", "1234", 1234]"#;
    assert!(!filter(f, &[]));
    assert!(filter_id(f, &[], Identifier::Uint(1234)));
    assert!(filter_id(f, &[], Identifier::String("1234".into())));
    assert!(!filter_id(f, &[], Identifier::String("4321".into())));
}

// gtest: Filter.Any
#[test]
fn any() {
    assert!(!filter(r#"["any"]"#, &[]));
    assert!(filter(r#"["any", ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["any", ["==", "foo", 0]]"#, &[("foo", Value::Int(1))]));
    assert!(filter(r#"["any", ["==", "foo", 0], ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
}

// gtest: Filter.AnyExpression
#[test]
fn any_expression() {
    assert!(!filter(r#"["any"]"#, &[]));
    assert!(filter(r#"["any", ["==", ["get", "foo"], 1]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["any", ["==", ["get", "foo"], 0]]"#, &[("foo", Value::Int(1))]));
    assert!(filter(r#"["any", ["==", ["get", "foo"], 0], ["==", ["get", "foo"], 1]]"#, &[("foo", Value::Int(1))]));
}

// gtest: Filter.All
#[test]
fn all() {
    assert!(filter(r#"["all"]"#, &[]));
    assert!(filter(r#"["all", ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["all", ["==", "foo", 0]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["all", ["==", "foo", 0], ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
}

// gtest: Filter.AllExpression
#[test]
fn all_expression() {
    assert!(filter(r#"["all", ["==", ["get", "foo"], 1]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["all", ["==", ["get", "foo"], 0]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["all", ["==", ["get", "foo"], 0], ["==", ["get", "foo"], 1]]"#, &[("foo", Value::Int(1))]));
}

// gtest: Filter.None
#[test]
fn none() {
    assert!(filter(r#"["none"]"#, &[]));
    assert!(!filter(r#"["none", ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
    assert!(filter(r#"["none", ["==", "foo", 0]]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["none", ["==", "foo", 0], ["==", "foo", 1]]"#, &[("foo", Value::Int(1))]));
}

// gtest: Filter.Has
#[test]
fn has() {
    assert!(filter(r#"["has", "foo"]"#, &[("foo", Value::Int(1))]));
    assert!(filter(r#"["has", "foo"]"#, &[("foo", Value::Int(0))]));
    assert!(filter(r#"["has", "foo"]"#, &[("foo", Value::Bool(false))]));
    assert!(!filter(r#"["has", "foo"]"#, &[]));
    assert!(!filter(r#"["has", "$id"]"#, &[]));
}

// gtest: Filter.NotHas
#[test]
fn not_has() {
    assert!(!filter(r#"["!has", "foo"]"#, &[("foo", Value::Int(1))]));
    assert!(!filter(r#"["!has", "foo"]"#, &[("foo", Value::Int(0))]));
    assert!(!filter(r#"["!has", "foo"]"#, &[("foo", Value::Bool(false))]));
    assert!(filter(r#"["!has", "foo"]"#, &[]));
}

// gtest: Filter.ID
#[test]
fn id() {
    let id1 = Identifier::Uint(1234);
    assert!(filter_id(r#"["==", "$id", 1234]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["==", "$id", "1234"]"#, &[], id1.clone()));
    assert!(!filter(r#"["==", "$id", 1234]"#, &[("id", Value::Uint(1234))]));
    assert!(!filter(r#"["==", "$id", "1234"]"#, &[("id", Value::Uint(1234))]));

    assert!(!filter_id(r#"["<", "$id", 0]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["<", "$id", 1234]"#, &[], id1.clone()));
    assert!(filter_id(r#"["<=", "$id", 1234]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["<", "$id", 123]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["<=", "$id", 123]"#, &[], id1.clone()));
    assert!(filter_id(r#"[">", "$id", 0]"#, &[], id1.clone()));
    assert!(filter_id(r#"[">", "$id", 123]"#, &[], id1.clone()));
    assert!(filter_id(r#"[">=", "$id", 123]"#, &[], id1.clone()));
    assert!(!filter_id(r#"[">", "$id", 1234]"#, &[], id1.clone()));
    assert!(filter_id(r#"[">=", "$id", 1234]"#, &[], id1.clone()));

    assert!(!filter_id(r#"[">", "$id", "1"]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["<", "$id", "1"]"#, &[], id1.clone()));
    assert!(!filter_id(r#"[">=", "$id", "1"]"#, &[], id1.clone()));
    assert!(!filter_id(r#"["<=", "$id", "1"]"#, &[], id1.clone()));

    let id2 = Identifier::String("1".into());
    assert!(!filter_id(r#"["<", "$id", "0"]"#, &[], id2.clone()));
    assert!(filter_id(r#"["<", "$id", "1234"]"#, &[], id2.clone()));
    assert!(filter_id(r#"["<=", "$id", "1234"]"#, &[], id2.clone()));
    assert!(!filter_id(r#"["<", "$id", "012"]"#, &[], id2.clone()));
    assert!(!filter_id(r#"["<=", "$id", "012"]"#, &[], id2.clone()));
    assert!(filter_id(r#"[">", "$id", "0"]"#, &[], id2.clone()));
    assert!(!filter_id(r#"[">", "$id", "234"]"#, &[], id2.clone()));
    assert!(filter_id(r#"[">=", "$id", "012"]"#, &[], id2.clone()));
    assert!(!filter_id(r#"[">", "$id", "1234"]"#, &[], id2.clone()));
    assert!(!filter_id(r#"[">=", "$id", "1234"]"#, &[], id2.clone()));

    assert!(!filter_id(r#"[">", "$id", 1]"#, &[], id2.clone()));
    assert!(!filter_id(r#"["<", "$id", 1]"#, &[], id2.clone()));
    assert!(!filter_id(r#"[">=", "$id", 1]"#, &[], id2.clone()));
    assert!(!filter_id(r#"["<=", "$id", 1]"#, &[], id2));
}

// gtest: Filter.Expression
#[test]
fn expression() {
    assert!(filter(r#"["==", ["+", 1, 1], 2]"#, &[]));
    assert!(!filter(r#"["==", ["+", 1, 1], 4]"#, &[]));
}

// gtest: Filter.PropertyExpression
#[test]
fn property_expression() {
    assert!(filter(r#"["==", ["get", "two"], 2]"#, &[("two", Value::Int(2))]));
    assert!(!filter(r#"["==", ["get", "two"], 4]"#, &[("two", Value::Int(2))]));
}

// gtest: Filter.LegacyProperty
#[test]
fn legacy_property() {
    let i = Value::Int;
    // <

    assert!(filter(r#"["<", "two", 2]"#, &[("two", i(1))]));
    assert!(!filter(r#"["<", "two", "2"]"#, &[("two", i(1))]));
    assert!(!filter(r#"["<", "two", 2]"#, &[("two", s("1"))]));
    assert!(filter(r#"["<", "two", "2"]"#, &[("two", s("1"))]));

    assert!(!filter(r#"["<", "two", 2]"#, &[("two", i(2))]));
    assert!(!filter(r#"["<", "two", "2"]"#, &[("two", i(2))]));
    assert!(!filter(r#"["<", "two", 2]"#, &[("two", s("2"))]));
    assert!(!filter(r#"["<", "two", "2"]"#, &[("two", s("2"))]));

    // <=

    assert!(filter(r#"["<=", "two", 2]"#, &[("two", i(1))]));
    assert!(!filter(r#"["<=", "two", "2"]"#, &[("two", i(1))]));
    assert!(!filter(r#"["<=", "two", 2]"#, &[("two", s("1"))]));
    assert!(filter(r#"["<=", "two", "2"]"#, &[("two", s("1"))]));

    assert!(filter(r#"["<=", "two", 2]"#, &[("two", i(2))]));
    assert!(!filter(r#"["<=", "two", "2"]"#, &[("two", i(2))]));
    assert!(!filter(r#"["<=", "two", 2]"#, &[("two", s("2"))]));
    assert!(filter(r#"["<=", "two", "2"]"#, &[("two", s("2"))]));

    assert!(!filter(r#"["<=", "two", 2]"#, &[("two", i(3))]));
    assert!(!filter(r#"["<=", "two", "2"]"#, &[("two", i(3))]));
    assert!(!filter(r#"["<=", "two", 2]"#, &[("two", s("3"))]));
    assert!(!filter(r#"["<=", "two", "2"]"#, &[("two", s("3"))]));

    // >

    assert!(!filter(r#"[">", "two", 2]"#, &[("two", i(2))]));
    assert!(!filter(r#"[">", "two", "2"]"#, &[("two", i(2))]));
    assert!(!filter(r#"[">", "two", 2]"#, &[("two", s("2"))]));
    assert!(!filter(r#"[">", "two", "2"]"#, &[("two", s("2"))]));

    assert!(filter(r#"[">", "two", 2]"#, &[("two", i(3))]));
    assert!(!filter(r#"[">", "two", "2"]"#, &[("two", i(3))]));
    assert!(!filter(r#"[">", "two", 2]"#, &[("two", s("3"))]));
    assert!(filter(r#"[">", "two", "2"]"#, &[("two", s("3"))]));

    // >=

    assert!(!filter(r#"[">=", "two", 2]"#, &[("two", i(1))]));
    assert!(!filter(r#"[">=", "two", "2"]"#, &[("two", i(1))]));
    assert!(!filter(r#"[">=", "two", 2]"#, &[("two", s("1"))]));
    assert!(!filter(r#"[">=", "two", "2"]"#, &[("two", s("1"))]));

    assert!(filter(r#"[">=", "two", 2]"#, &[("two", i(2))]));
    assert!(!filter(r#"[">=", "two", "2"]"#, &[("two", i(2))]));
    assert!(!filter(r#"[">=", "two", 2]"#, &[("two", s("2"))]));
    assert!(filter(r#"[">=", "two", "2"]"#, &[("two", s("2"))]));

    assert!(filter(r#"[">=", "two", 2]"#, &[("two", i(3))]));
    assert!(!filter(r#"[">=", "two", "2"]"#, &[("two", i(3))]));
    assert!(!filter(r#"[">=", "two", 2]"#, &[("two", s("3"))]));
    assert!(filter(r#"[">=", "two", "2"]"#, &[("two", s("3"))]));

    //

    assert!(filter(r#"["==", "two", 4]"#, &[("two", i(4))]));
    assert!(!filter(r#"["==", "two", 4]"#, &[("two", s("4"))]));
    assert!(!filter(r#"["==", "two", "4"]"#, &[("two", i(4))]));
    assert!(filter(r#"["==", "two", "4"]"#, &[("two", s("4"))]));

    assert!(!filter(r#"["==", "two", 4]"#, &[("two", i(2))]));
    assert!(!filter(r#"["==", "two", 4]"#, &[("two", s("2"))]));
    assert!(!filter(r#"["==", "two", "4"]"#, &[("two", i(2))]));
    assert!(!filter(r#"["==", "two", "4"]"#, &[("two", s("2"))]));

    assert!(!filter(r#"["==", "one", true]"#, &[("two", Value::Bool(true))]));
    assert!(!filter(r#"["==", "one", false]"#, &[("two", Value::Bool(true))]));
    assert!(filter(r#"["==", "two", true]"#, &[("two", Value::Bool(true))]));
    assert!(!filter(r#"["==", "two", false]"#, &[("two", Value::Bool(true))]));
}

// gtest: Filter.Serialize
#[test]
fn serialize() {
    let json = r#"["any",["==","foo",0],["==","foo",1]]"#;
    assert_eq!(stringify_filter(json), json);

    let json = r#"["<=","two",2]"#;
    assert_eq!(stringify_filter(json), json);

    // Constant folding for Expression filters
    let json = r#"["==",["+",1,1],2]"#;
    assert_eq!(stringify_filter(json), "true");

    let json = r#"["all",["==",["get","foo"],0],["==",["get","foo"],1]]"#;
    assert_eq!(stringify_filter(json), json);
}

// gtest: Filter.ExpressionLegacyMix
#[test]
fn expression_legacy_mix() {
    let mut error = Error::default();
    let filter =
        convert_json(r#"["any", ["all", ["==", ["geometry-type"], "LineString"]], ["==", "x", 1]]"#, &mut error);
    assert!(filter.is_none());
    assert_ne!(error.message, "");
}

// gtest: Filter.ZoomExpressionNested
#[test]
fn zoom_expression_nested() {
    assert!(filter_full(
        r#"["==", ["get", "two"], ["zoom"]]"#,
        &[("two", Value::Int(2))],
        Identifier::Null,
        FeatureType::Point,
        2.0
    ));
    assert!(!filter_full(
        r#"["==", ["get", "two"], ["+", ["zoom"], 1]]"#,
        &[("two", Value::Int(2))],
        Identifier::Null,
        FeatureType::Point,
        2.0
    ));
}

// gtest: Filter.GeomType
#[test]
fn geom_type() {
    let check = |json: &str, type_: FeatureType| filter_full(json, &[], Identifier::Null, type_, 2.0);
    assert!(check(r#"["==", ["geometry-type"], "Point"]"#, FeatureType::Point));
    assert!(!check(r#"["==", ["geometry-type"], "LineString"]"#, FeatureType::Point));
    assert!(!check(r#"["==", ["geometry-type"], "Polygon"]"#, FeatureType::Point));
    assert!(!check(r#"["==", ["geometry-type"], "Unknown"]"#, FeatureType::Point));

    assert!(!check(r#"["==", ["geometry-type"], "Point"]"#, FeatureType::LineString));
    assert!(check(r#"["==", ["geometry-type"], "LineString"]"#, FeatureType::LineString));
    assert!(!check(r#"["==", ["geometry-type"], "Polygon"]"#, FeatureType::LineString));
    assert!(!check(r#"["==", ["geometry-type"], "Unknown"]"#, FeatureType::LineString));

    assert!(!check(r#"["==", ["geometry-type"], "Point"]"#, FeatureType::Polygon));
    assert!(!check(r#"["==", ["geometry-type"], "LineString"]"#, FeatureType::Polygon));
    assert!(check(r#"["==", ["geometry-type"], "Polygon"]"#, FeatureType::Polygon));
    assert!(!check(r#"["==", ["geometry-type"], "Unknown"]"#, FeatureType::Polygon));

    assert!(!check(r#"["==", ["geometry-type"], "Point"]"#, FeatureType::Unknown));
    assert!(!check(r#"["==", ["geometry-type"], "LineString"]"#, FeatureType::Unknown));
    assert!(!check(r#"["==", ["geometry-type"], "Polygon"]"#, FeatureType::Unknown));
    assert!(check(r#"["==", ["geometry-type"], "Unknown"]"#, FeatureType::Unknown));
}

// gtest: Filter.Internal
#[test]
fn internal() {
    filter(r#"["filter-==","class","snow"]"#, &[]);
}

// gtest: Filter.Short
#[test]
fn short() {
    filter(r#"["==", ["id"], "foo"]"#, &[]);
}

// gtest: Filter.LegacyExpressionInvalidType
#[test]
fn legacy_expression_invalid_type() {
    let value = serde_json::json!("string");
    let mut error = Error::default();
    let result = <Filter as Converter>::convert(&Convertible::from(&value), &mut error);
    assert!(result.is_none());
}

// Port-only: `isExpression` reads `["in"]`'s second member past the end (an
// assert in C++, undefined behaviour with NDEBUG). Here it is null, so the
// filter is a legacy one, and a one-element legacy filter passes.
#[test]
fn in_without_operands() {
    assert!(filter(r#"["in"]"#, &[]));
    assert!(filter(r#"["any", ["in"]]"#, &[]));
}
