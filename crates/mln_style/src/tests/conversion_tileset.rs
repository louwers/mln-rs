use mln_conversion::Convertible;
use mln_style_types::SourceType;
use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::tileset::{PartialTileset, Scheme};

use super::util::parse_json;
use crate::conversion::tileset::convert_tileset;

fn convert_json(json: &str) -> Result<PartialTileset, String> {
    convert_tileset(&Convertible::from(&parse_json(json)), SourceType::Vector)
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

// gtest: Tileset.Empty
#[test]
fn empty() {
    // A source without tiles needs a URL; a TileJSON's members are all optional.
    assert_eq!(Ok(PartialTileset::default()), convert_json("{}"));
}

// gtest: Tileset.ErrorHandling
#[test]
fn error_handling() {
    let converted = convert_json(
        r#"{
        "tiles": "should not be a string"
    }"#,
    );
    assert!(converted.is_err());
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
        let converted = convert_json(json);
        assert!(converted.is_err(), "{json}");
    }
}

// gtest: Tileset.ValidWorldBounds
#[test]
fn valid_world_bounds() {
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [-180, -90, 180, 90]
    }"#,
    );
    assert!(converted.is_ok());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(90.0, -180.0), ll(-90.0, 180.0))));
}

// gtest: Tileset.PointBounds
#[test]
fn point_bounds() {
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [0, 0, 0, 0]
    }"#,
    );
    assert!(converted.is_ok());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(0.0, 0.0), ll(0.0, 0.0))));
}

// gtest: Tileset.BoundsAreClamped
#[test]
fn bounds_are_clamped() {
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "bounds": [-181.0000005,-90.000000006,180.00000000000006,91]
    }"#,
    );
    assert!(converted.is_ok());
    assert_eq!(converted.unwrap().bounds, Some(LatLngBounds::hull(ll(90.0, -180.0), ll(-90.0, 180.0))));
}

// gtest: Tileset.FullConversion
#[test]
fn full_conversion() {
    let converted = convert_json(
        r#"{
        "tiles": ["http://mytiles"],
        "scheme": "xyz",
        "minzoom": 1,
        "maxzoom": 2,
        "attribution": "mapbox",
        "bounds": [-180, -73, -120, 73]
    }"#,
    )
    .unwrap();

    assert_eq!(converted.tiles, Some(vec!["http://mytiles".to_owned()]));
    assert_eq!(converted.scheme, Some(Scheme::XYZ));
    assert_eq!(converted.min_zoom, Some(1));
    assert_eq!(converted.max_zoom, Some(2));
    assert_eq!(converted.attribution.as_deref(), Some("mapbox"));
    assert_eq!(converted.bounds, Some(LatLngBounds::hull(ll(73.0, -180.0), ll(-73.0, -120.0))));
}
