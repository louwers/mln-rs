// Ports vendor/maplibre-native-base/deps/geojson.hpp/test/test.cpp over its
// fixtures. `use_convert` reads through `convert` (C++: a rapidjson
// document) instead of `parse`, and writes through `to_json` + serde_json
// instead of `stringify`.

use std::path::PathBuf;

use geo_types::Geometry;

use super::{GeoJSON, convert, convert_geometry, parse, stringify, stringify_geometry, to_json, to_json_geometry};
use crate::{Feature, Identifier, Value};

fn fixture(name: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/vendor/maplibre-native-base/deps/geojson.hpp/test/fixtures")
        .join(name)
}

fn read_file(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

fn read_geojson(name: &str, use_convert: bool) -> Result<GeoJSON, super::Error> {
    let text = read_file(name);
    if use_convert {
        let d: serde_json::Value = serde_json::from_str(&text).unwrap();
        convert(&d)
    } else {
        parse(&text)
    }
}

fn write_geojson(data: &GeoJSON, use_convert: bool) -> String {
    if use_convert { serde_json::to_string(&to_json(data)).unwrap() } else { stringify(data) }
}

fn geometry(data: &GeoJSON) -> &Geometry<f64> {
    match data {
        GeoJSON::Geometry(g) => g,
        _ => panic!("not a geometry"),
    }
}

fn feature(data: &GeoJSON) -> &Feature<f64> {
    match data {
        GeoJSON::Feature(f) => f,
        _ => panic!("not a feature"),
    }
}

fn roundtrips(data: &GeoJSON, use_convert: bool) {
    assert_eq!(&parse(&write_geojson(data, use_convert)).unwrap(), data);
}

#[test]
fn test_empty() {
    let d: serde_json::Value = serde_json::from_str(&read_file("null.json")).unwrap();
    let geom = convert_geometry(&d).unwrap();
    assert_eq!(geom, None);
    assert_eq!(stringify_geometry(geom.as_ref()), "null");
    assert_eq!(to_json_geometry(geom.as_ref()), serde_json::Value::Null);
}

fn test_point(use_convert: bool) {
    let data = read_geojson("point.json", use_convert).unwrap();
    let Geometry::Point(p) = geometry(&data) else { panic!() };
    assert_eq!(p.x(), 30.5);
    assert_eq!(p.y(), 50.5);
    roundtrips(&data, use_convert);
}

fn test_multi_point(use_convert: bool) {
    let data = read_geojson("multi-point.json", use_convert).unwrap();
    let Geometry::MultiPoint(points) = geometry(&data) else { panic!() };
    assert_eq!(points.0.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_line_string(use_convert: bool) {
    let data = read_geojson("line-string.json", use_convert).unwrap();
    let Geometry::LineString(points) = geometry(&data) else { panic!() };
    assert_eq!(points.0.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_multi_line_string(use_convert: bool) {
    let data = read_geojson("multi-line-string.json", use_convert).unwrap();
    let Geometry::MultiLineString(lines) = geometry(&data) else { panic!() };
    assert_eq!(lines.0.len(), 1);
    assert_eq!(lines.0[0].0.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_polygon(use_convert: bool) {
    let data = read_geojson("polygon.json", use_convert).unwrap();
    let Geometry::Polygon(polygon) = geometry(&data) else { panic!() };
    assert_eq!(polygon.interiors(), []);
    assert_eq!(polygon.exterior().0.len(), 5);
    assert_eq!(polygon.exterior().0[0], polygon.exterior().0[4]);
    roundtrips(&data, use_convert);
}

fn test_multi_polygon(use_convert: bool) {
    let data = read_geojson("multi-polygon.json", use_convert).unwrap();
    let Geometry::MultiPolygon(polygons) = geometry(&data) else { panic!() };
    assert_eq!(polygons.0.len(), 1);
    assert_eq!(polygons.0[0].interiors(), []);
    let exterior = &polygons.0[0].exterior().0;
    assert_eq!(exterior.len(), 5);
    assert_eq!(exterior[0], exterior[4]);
    roundtrips(&data, use_convert);
}

fn test_geometry_collection(use_convert: bool) {
    let data = read_geojson("geometry-collection.json", use_convert).unwrap();
    let Geometry::GeometryCollection(collection) = geometry(&data) else { panic!() };
    assert!(matches!(collection.0[0], Geometry::Point(_)));
    assert!(matches!(collection.0[1], Geometry::LineString(_)));
    roundtrips(&data, use_convert);
}

fn test_feature(use_convert: bool) {
    let data = read_geojson("feature.json", use_convert).unwrap();
    let f = feature(&data);
    assert!(matches!(f.geometry, Some(Geometry::Point(_))));

    assert_eq!(f.properties["bool"], Value::Bool(true));
    assert_eq!(f.properties["string"], Value::String("foo".to_owned()));
    assert_eq!(f.properties["double"], Value::Double(2.5));
    assert_eq!(f.properties["uint"], Value::Uint(10));
    assert_eq!(f.properties["int"], Value::Int(-10));
    assert_eq!(f.properties["null"], Value::Null);

    let Value::Array(nested) = &f.properties["nested"] else { panic!() };
    assert_eq!(nested[0], Value::Uint(5));
    let Value::Object(object) = &nested[1] else { panic!() };
    assert_eq!(object["foo"], Value::String("bar".to_owned()));

    roundtrips(&data, use_convert);
}

fn test_feature_null_properties(use_convert: bool) {
    let data = read_geojson("feature-null-properties.json", use_convert).unwrap();
    let f = feature(&data);
    assert!(matches!(f.geometry, Some(Geometry::Point(_))));
    assert!(f.properties.is_empty());
    roundtrips(&data, use_convert);
}

fn test_feature_null_geometry(use_convert: bool) {
    let data = read_geojson("feature-null-geometry.json", use_convert).unwrap();
    let f = feature(&data);
    assert_eq!(f.geometry, None);
    assert!(f.properties.is_empty());
    roundtrips(&data, use_convert);
}

fn test_feature_missing_properties(use_convert: bool) {
    let data = read_geojson("feature-missing-properties.json", use_convert).unwrap();
    let f = feature(&data);
    assert!(matches!(f.geometry, Some(Geometry::Point(_))));
    assert!(f.properties.is_empty());
    roundtrips(&data, use_convert);
}

fn test_feature_collection(use_convert: bool) {
    let data = read_geojson("feature-collection.json", use_convert).unwrap();
    let GeoJSON::FeatureCollection(features) = &data else { panic!() };
    assert_eq!(features.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_feature_id(use_convert: bool) {
    let data = read_geojson("feature-id.json", use_convert).unwrap();
    let GeoJSON::FeatureCollection(features) = &data else { panic!() };
    assert_eq!(features[0].id, Identifier::Uint(1234));
    assert_eq!(features[1].id, Identifier::String("abcd".to_owned()));
    roundtrips(&data, use_convert);
}

fn error_message(name: &str) -> String {
    read_geojson(name, false).expect_err("Should have thrown an error").0
}

#[test]
fn test_parse_error_handling() {
    // C++: rapidjson's "Invalid value." (contains "Invalid"); the port
    // reports serde_json's syntax error.
    assert!(error_message("invalid.json").contains("expected value"));

    // test invalid polygon
    assert!(error_message("invalid-polygon.json").contains("described by 4"));
    // C++: "coordinates must be an array." (contains "array"); the
    // `geojson` crate reports the nesting depth.
    assert!(error_message("invalid-polygon-2.json").contains("unexpected coordinates dimensions"));
    assert!(error_message("invalid-multi-polygon.json").contains("described by 4"));
    assert!(error_message("invalid-multi-polygon-2.json").contains("2 numbers"));
    assert!(error_message("invalid-line-string.json").contains("two or more"));
    assert!(error_message("invalid-multi-line-string.json").contains("two or more"));
}

fn test_all(use_convert: bool) {
    test_point(use_convert);
    test_multi_point(use_convert);
    test_line_string(use_convert);
    test_multi_line_string(use_convert);
    test_polygon(use_convert);
    test_multi_polygon(use_convert);
    test_geometry_collection(use_convert);
    test_feature(use_convert);
    test_feature_null_properties(use_convert);
    test_feature_null_geometry(use_convert);
    test_feature_missing_properties(use_convert);
    test_feature_collection(use_convert);
    test_feature_id(use_convert);
}

#[test]
fn test_all_convert() {
    test_all(true);
}

#[test]
fn test_all_parse() {
    test_all(false);
}
