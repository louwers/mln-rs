use mln_conversion::Convertible;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tileset::{Scheme, Tileset};

use super::util::parse_json;
use crate::conversion::{Converter, Error};

fn convert_json(json: &str, error: &mut Error) -> Option<Tileset> {
    let value = parse_json(json);
    <Tileset as Converter>::convert(&Convertible::from(&value), error)
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

// gtest: Tileset.Empty
#[test]
fn empty() {
    let mut error = Error::default();
    let converted = convert_json("{}", &mut error);
    assert!(converted.is_none());
}

// gtest: Tileset.ErrorHandling
#[test]
fn error_handling() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "tiles": "should not be a string"
    }"#,
        &mut error,
    );
    assert!(converted.is_none());
}

// gtest: Tileset.InvalidBounds
#[test]
fn invalid_bounds() {
    for json in [
        r#"{
            "tiles": ["http://mytiles"],
            "bounds": [73, -180, -73, -120]
        }"#,
        r#"{
            "tiles": ["http://mytiles"],
            "bounds": [-120]
        }"#,
        r#"{
            "tiles": ["http://mytiles"],
            "bounds": "should not be a string"
        }"#,
    ] {
        let mut error = Error::default();
        let converted = convert_json(json, &mut error);
        assert!(converted.is_none(), "{json}");
    }
}

// gtest: Tileset.ValidWorldBounds
#[test]
fn valid_world_bounds() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [-180, -90, 180, 90]
    }"#,
        &mut error,
    );
    assert!(converted.is_some());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(90.0, -180.0), ll(-90.0, 180.0))));
}

// gtest: Tileset.PointBounds
#[test]
fn point_bounds() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [0, 0, 0, 0]
    }"#,
        &mut error,
    );
    assert!(converted.is_some());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(0.0, 0.0), ll(0.0, 0.0))));
}

// gtest: Tileset.BoundsAreClamped
#[test]
fn bounds_are_clamped() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [-181.0000005,-90.000000006,180.00000000000006,91]
    }"#,
        &mut error,
    );
    assert!(converted.is_some());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(90.0, -180.0), ll(-90.0, 180.0))));
}

// gtest: Tileset.FullConversion
#[test]
fn full_conversion() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "scheme": "xyz",
        "minzoom": 1,
        "maxzoom": 2,
        "attribution": "mapbox",
        "bounds": [-180, -73, -120, 73]
    }"#,
        &mut error,
    )
    .unwrap();

    assert_eq!(converted.tiles[0], "http://mytiles");
    assert_eq!(converted.scheme, Scheme::XYZ);
    assert_eq!(converted.zoom_range.min, 1);
    assert_eq!(converted.zoom_range.max, 2);
    assert_eq!(converted.attribution, "mapbox");
    assert_eq!(converted.bounds, Some(LatLngBounds::hull(ll(73.0, -180.0), ll(-73.0, -120.0))));
}
