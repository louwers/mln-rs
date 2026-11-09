// Ports vendor/maplibre-native-base/deps/geojson.hpp/test/test.cpp over its
// fixtures, read in place. `use_convert` reads through `convert` (C++: a
// rapidjson document) instead of `parse`, and writes through `to_json` +
// serde_json instead of `stringify`.
// TODO(port): test/test_value.cpp tests geojson_value_impl.hpp, which the
// core does not compile (not ported).

use std::path::PathBuf;

use geojson::{
    Feature, GeoJSON, Geometry, Identifier, Value, convert, convert_geometry, parse, stringify, stringify_geometry,
    to_json, to_json_geometry,
};

fn fixture(name: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/vendor/maplibre-native-base/deps/geojson.hpp/test/fixtures")
        .join(name)
}

fn read_file(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

fn read_geojson(name: &str, use_convert: bool) -> Result<GeoJSON, geojson::Error> {
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
    assert_eq!(geom, Geometry::Empty);
    assert_eq!(stringify_geometry(&geom), "null");
    assert_eq!(to_json_geometry(&geom), serde_json::Value::Null);
}

fn test_point(use_convert: bool) {
    let data = read_geojson("point.json", use_convert).unwrap();
    let Geometry::Point(p) = geometry(&data) else { panic!() };
    assert_eq!(p.x, 30.5);
    assert_eq!(p.y, 50.5);
    roundtrips(&data, use_convert);
}

fn test_multi_point(use_convert: bool) {
    let data = read_geojson("multi-point.json", use_convert).unwrap();
    let Geometry::MultiPoint(points) = geometry(&data) else { panic!() };
    assert_eq!(points.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_line_string(use_convert: bool) {
    let data = read_geojson("line-string.json", use_convert).unwrap();
    let Geometry::LineString(points) = geometry(&data) else { panic!() };
    assert_eq!(points.len(), 2);
    roundtrips(&data, use_convert);
}

fn test_multi_line_string(use_convert: bool) {
    let data = read_geojson("multi-line-string.json", use_convert).unwrap();
    let Geometry::MultiLineString(lines) = geometry(&data) else { panic!() };
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].len(), 2);
    roundtrips(&data, use_convert);
}

fn test_polygon(use_convert: bool) {
    let data = read_geojson("polygon.json", use_convert).unwrap();
    let Geometry::Polygon(rings) = geometry(&data) else { panic!() };
    assert_eq!(rings.len(), 1);
    assert_eq!(rings[0].len(), 5);
    assert_eq!(rings[0][0], rings[0][4]);
    roundtrips(&data, use_convert);
}

fn test_multi_polygon(use_convert: bool) {
    let data = read_geojson("multi-polygon.json", use_convert).unwrap();
    let Geometry::MultiPolygon(polygons) = geometry(&data) else { panic!() };
    assert_eq!(polygons.len(), 1);
    assert_eq!(polygons[0].len(), 1);
    assert_eq!(polygons[0][0].len(), 5);
    assert_eq!(polygons[0][0][0], polygons[0][0][4]);
    roundtrips(&data, use_convert);
}

fn test_geometry_collection(use_convert: bool) {
    let data = read_geojson("geometry-collection.json", use_convert).unwrap();
    let Geometry::GeometryCollection(collection) = geometry(&data) else { panic!() };
    assert!(matches!(collection[0], Geometry::Point(_)));
    assert!(matches!(collection[1], Geometry::LineString(_)));
    roundtrips(&data, use_convert);
}

fn test_feature(use_convert: bool) {
    let data = read_geojson("feature.json", use_convert).unwrap();
    let f = feature(&data);
    assert!(matches!(f.geometry, Geometry::Point(_)));

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
    assert!(matches!(f.geometry, Geometry::Point(_)));
    assert!(f.properties.is_empty());
    roundtrips(&data, use_convert);
}

fn test_feature_null_geometry(use_convert: bool) {
    let data = read_geojson("feature-null-geometry.json", use_convert).unwrap();
    let f = feature(&data);
    assert_eq!(f.geometry, Geometry::Empty);
    assert!(f.properties.is_empty());
    roundtrips(&data, use_convert);
}

fn test_feature_missing_properties(use_convert: bool) {
    let data = read_geojson("feature-missing-properties.json", use_convert).unwrap();
    let f = feature(&data);
    assert!(matches!(f.geometry, Geometry::Point(_)));
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
    assert!(error_message("invalid-polygon-2.json").contains("array"));
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
