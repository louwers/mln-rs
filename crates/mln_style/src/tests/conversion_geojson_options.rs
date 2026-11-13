use mln_conversion::Convertible;

use super::util::parse_json;
use crate::conversion::{Converter, Error};
use crate::sources::geojson_source::GeoJSONOptions;

fn convert_json(json: &str, error: &mut Error) -> Option<GeoJSONOptions> {
    let value = parse_json(json);
    <GeoJSONOptions as Converter>::convert(&Convertible::from(&value), error)
}

// gtest: GeoJSONOptions.Basic
#[test]
fn basic() {
    let mut error = Error::default();
    let converted = convert_json("{}", &mut error);
    assert!(converted.is_some());
}

// gtest: GeoJSONOptions.ErrorHandling
#[test]
fn error_handling() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "maxzoom": "should not be a string"
    }"#,
        &mut error,
    );
    assert!(converted.is_none());
}

// gtest: GeoJSONOptions.RetainsDefaults
#[test]
fn retains_defaults() {
    let mut error = Error::default();
    let converted = convert_json("{}", &mut error).unwrap();
    let defaults = GeoJSONOptions::default();

    // GeoJSON-VT
    assert_eq!(converted.minzoom, defaults.minzoom);
    assert_eq!(converted.maxzoom, defaults.maxzoom);
    assert_eq!(converted.buffer, defaults.buffer);
    assert_eq!(converted.tolerance, defaults.tolerance);
    assert_eq!(converted.line_metrics, defaults.line_metrics);

    // Supercluster
    assert_eq!(converted.cluster, defaults.cluster);
    assert_eq!(converted.cluster_radius, defaults.cluster_radius);
    assert_eq!(converted.cluster_max_zoom, defaults.cluster_max_zoom);
    assert_eq!(converted.cluster_min_points, defaults.cluster_min_points);
    assert!(converted.cluster_properties.is_empty());
}

// gtest: GeoJSONOptions.FullConversion
#[test]
fn full_conversion() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "maxzoom": 1,
        "buffer": 2,
        "tolerance": 3,
        "cluster": true,
        "clusterRadius": 4,
        "clusterMaxZoom": 5,
        "clusterMinPoints": 6,
        "lineMetrics": true,
        "clusterProperties": {
            "max": ["max", ["get", "scalerank"]],
            "sum": [["+", ["accumulated"], ["get", "sum"]], ["get", "scalerank"]],
            "has_island": ["any", ["==", ["get", "featureclass"], "island"]]
        }
    }"#,
        &mut error,
    )
    .unwrap_or_else(|| panic!("{}", error.message));

    // GeoJSON-VT
    assert_eq!(converted.minzoom, 0);
    assert_eq!(converted.maxzoom, 1);
    assert_eq!(converted.buffer, 2);
    assert_eq!(converted.tolerance, 3.0);
    assert!(converted.line_metrics);

    // Supercluster
    assert!(converted.cluster);
    assert_eq!(converted.cluster_radius, 4);
    assert_eq!(converted.cluster_max_zoom, 5);
    assert_eq!(converted.cluster_min_points, 6);
    assert_eq!(converted.cluster_properties.len(), 3);
    assert!(converted.cluster_properties.contains_key("max"));
    assert!(converted.cluster_properties.contains_key("sum"));
    assert!(converted.cluster_properties.contains_key("has_island"));
}
