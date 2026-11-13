use mln_util::geo::{LatLng, LatLngBounds};
use mln_util::geometry::{Geometry, Point};

use crate::offline::{
    OfflineGeometryRegionDefinition, OfflineRegionDefinition, OfflineTilePyramidRegionDefinition,
    decode_offline_region_definition, encode_offline_region_definition,
};

// gtest: OfflineTilePyramidRegionDefinition.EncodeDecode
#[test]
fn offline_tile_pyramid_region_definition_encode_decode() {
    let region = OfflineTilePyramidRegionDefinition::new(
        "https://example.com/style",
        LatLngBounds::hull(LatLng::new(37.6609, -122.5744).unwrap(), LatLng::new(37.8271, -122.3204).unwrap()),
        0.0,
        20.0,
        1.0,
        true,
    )
    .unwrap();

    let encoded = encode_offline_region_definition(&region.clone().into());
    let OfflineRegionDefinition::TilePyramid(decoded) = decode_offline_region_definition(encoded.as_bytes()).unwrap()
    else {
        panic!("std::get<OfflineTilePyramidRegionDefinition>: bad variant access");
    };

    assert_eq!(decoded.style_url, region.style_url);
    assert_eq!(decoded.min_zoom, region.min_zoom);
    assert_eq!(decoded.max_zoom, region.max_zoom);
    assert_eq!(decoded.pixel_ratio, region.pixel_ratio);
    assert_eq!(decoded.bounds.southwest(), region.bounds.southwest());
    assert_eq!(decoded.bounds.northeast(), region.bounds.northeast());
    assert_eq!(decoded.include_ideographs, region.include_ideographs);
}

// gtest: OfflineGeometryRegionDefinition.EncodeDecode
#[test]
fn offline_geometry_region_definition_encode_decode() {
    let region = OfflineGeometryRegionDefinition::new(
        "https://example.com/style",
        Geometry::Point(Point::new(-122.5744, 37.6609)),
        0.0,
        2.0,
        1.0,
        false,
    )
    .unwrap();

    let encoded = encode_offline_region_definition(&region.clone().into());
    let OfflineRegionDefinition::Geometry(decoded) = decode_offline_region_definition(encoded.as_bytes()).unwrap()
    else {
        panic!("std::get<OfflineGeometryRegionDefinition>: bad variant access");
    };

    assert_eq!(decoded.style_url, region.style_url);
    assert_eq!(decoded.min_zoom, region.min_zoom);
    assert_eq!(decoded.max_zoom, region.max_zoom);
    assert_eq!(decoded.pixel_ratio, region.pixel_ratio);
    assert_eq!(decoded.geometry, region.geometry);
    assert_eq!(decoded.include_ideographs, region.include_ideographs);
}

// The members are in C++'s order.
#[test]
fn encoding() {
    let region = OfflineTilePyramidRegionDefinition::new(
        "https://example.com/style",
        LatLngBounds::hull(LatLng::new(37.6609, -122.5744).unwrap(), LatLng::new(37.8271, -122.3204).unwrap()),
        0.0,
        20.0,
        1.0,
        true,
    )
    .unwrap();
    assert_eq!(
        r#"{"style_url":"https://example.com/style","min_zoom":0.0,"max_zoom":20.0,"pixel_ratio":1.0,"include_ideographs":true,"bounds":[37.6609,-122.5744,37.8271,-122.3204]}"#,
        encode_offline_region_definition(&region.into())
    );

    let region = OfflineGeometryRegionDefinition::new(
        "https://example.com/style",
        Geometry::Point(Point::new(-122.5744, 37.6609)),
        0.0,
        f64::INFINITY,
        2.0,
        false,
    )
    .unwrap();
    assert_eq!(
        r#"{"style_url":"https://example.com/style","min_zoom":0.0,"pixel_ratio":2.0,"include_ideographs":false,"geometry":{"type":"Point","coordinates":[-122.5744,37.6609]}}"#,
        encode_offline_region_definition(&region.into())
    );
}

#[test]
fn decode_rejects_malformed_definitions() {
    for malformed in [
        "",
        "{}",
        r#"{"style_url":"s","min_zoom":0,"pixel_ratio":1.0,"bounds":[1.0,2.0,3.0,4.0]}"#,
        r#"{"style_url":"s","min_zoom":0.0,"pixel_ratio":1.0,"bounds":[1.0,2.0,3.0]}"#,
        r#"{"style_url":"s","min_zoom":0.0,"pixel_ratio":1.0,"bounds":[1.0,2.0,3.0,4.0],"include_ideographs":1}"#,
    ] {
        assert_eq!(
            "Malformed offline region definition",
            decode_offline_region_definition(malformed.as_bytes()).unwrap_err().message,
            "{malformed}"
        );
    }
    // Defaults: max_zoom = INFINITY, include_ideographs = true.
    let decoded = decode_offline_region_definition(
        br#"{"style_url":"s","min_zoom":1.0,"pixel_ratio":1.0,"bounds":[1.0,2.0,3.0,4.0]}"#,
    )
    .unwrap();
    assert_eq!(f64::INFINITY, decoded.max_zoom());
    assert!(decoded.include_ideographs());
}
