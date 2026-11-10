// Ports test/style/conversion/source_options.test.cpp.

use mln_conversion::Convertible;
use mln_util::tileset::{RasterEncoding, VectorEncoding};

use super::util::parse_json;
use crate::conversion::source::convert_source;
use crate::conversion::tileset::SourceOptions;
use crate::conversion::{Converter, Error};
use crate::source::Source;

/// `convertJSON<SourceOptions>(json, error)`.
fn convert_json(json: &str, error: &mut Error) -> Option<SourceOptions> {
    let value = parse_json(json);
    <SourceOptions as Converter>::convert(&Convertible::from(&value), error)
}

/// `convertJSON<std::unique_ptr<Source>>(json, error, id)`.
fn convert_source_json(json: &str, error: &mut Error, id: &str) -> Option<Box<dyn Source>> {
    let value = parse_json(json);
    convert_source(&Convertible::from(&value), error, id)
}

// gtest: SourceOptions.Basic
#[test]
fn basic() {
    let mut error = Error::default();
    let converted = convert_json("{}", &mut error);
    assert!(converted.is_none());
}

// gtest: SourceOptions.ErrorHandling
#[test]
fn error_handling() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "encoding": "this isn't a valid encoding"
    }"#,
        &mut error,
    );
    assert!(converted.is_none());
    assert_eq!(Some(0), error.message.find("invalid encoding"));
}

// gtest: SourceOptions.TerrariumEncodingParsed
#[test]
fn terrarium_encoding_parsed() {
    let mut error = Error::default();
    let converted = convert_json(r#"{"encoding": "terrarium"}"#, &mut error).unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Terrarium));
    assert!(converted.vector_encoding.is_none());
}

// gtest: SourceOptions.MapboxEncodingParsed
#[test]
fn mapbox_encoding_parsed() {
    let mut error = Error::default();
    let converted = convert_json(r#"{"encoding": "mapbox"}"#, &mut error).unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Mapbox));
    assert!(converted.vector_encoding.is_none());
}

// gtest: SourceOptions.MVTEncodingParsed
#[test]
fn mvt_encoding_parsed() {
    let mut error = Error::default();
    let converted = convert_json(r#"{"encoding": "mvt"}"#, &mut error).unwrap();
    assert_eq!(converted.vector_encoding, Some(VectorEncoding::Mapbox));
    assert!(converted.raster_encoding.is_none());
}

// gtest: SourceOptions.MLTEncodingParsed
#[test]
fn mlt_encoding_parsed() {
    let mut error = Error::default();
    let converted = convert_json(r#"{"encoding": "mlt"}"#, &mut error).unwrap();
    assert_eq!(converted.vector_encoding, Some(VectorEncoding::MLT));
    assert!(converted.raster_encoding.is_none());
}

// gtest: SourceOptions.ZoomRangeParsed
#[test]
fn zoom_range_parsed() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "minzoom": 5,
        "maxzoom": 14
    }"#,
        &mut error,
    )
    .unwrap();
    assert_eq!(converted.minzoom, Some(5));
    assert_eq!(converted.maxzoom, Some(14));
    assert!(converted.raster_encoding.is_none());
    assert!(converted.vector_encoding.is_none());
}

// gtest: SourceOptions.ZoomRangeAlongsideEncoding
#[test]
fn zoom_range_alongside_encoding() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "encoding": "terrarium",
        "maxzoom": 15
    }"#,
        &mut error,
    )
    .unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Terrarium));
    assert!(converted.minzoom.is_none());
    assert_eq!(converted.maxzoom, Some(15));
}

// gtest: SourceOptions.InvalidMaxzoomType
#[test]
fn invalid_maxzoom_type() {
    let mut error = Error::default();
    let converted = convert_json(
        r#"{
        "maxzoom": "high"
    }"#,
        &mut error,
    );
    assert!(converted.is_none());
    assert_eq!(Some(0), error.message.find("invalid maxzoom"));
}

// gtest: SourceOptions.ZoomOutOfRange
#[test]
fn zoom_out_of_range() {
    let mut error = Error::default();
    assert!(convert_json(r#"{ "minzoom": -1 }"#, &mut error).is_none());
    assert_eq!(Some(0), error.message.find("invalid minzoom"));
    assert!(convert_json(r#"{ "maxzoom": 300 }"#, &mut error).is_none());
    assert_eq!(Some(0), error.message.find("invalid maxzoom"));
}

// gtest: SourceOptions.RasterDEMSourceWithAnInvalidZoomIsRejected
#[test]
fn raster_dem_source_with_an_invalid_zoom_is_rejected() {
    // Through the style's source conversion: an invalid inline zoom rejects a raster-dem source
    // instead of building it without its options, which decoded a terrarium DEM as Mapbox with
    // nothing logged.
    let mut error = Error::default();
    let source = convert_source_json(
        r#"{ "type": "raster-dem", "url": "mbtiles://dem", "encoding": "terrarium", "maxzoom": "16" }"#,
        &mut error,
        "dem",
    );
    assert!(source.is_none());
    assert_eq!(Some(0), error.message.find("invalid maxzoom"));
}

// gtest: SourceOptions.RasterDEMSourceWithAnInvalidEncodingIsRejected
#[test]
fn raster_dem_source_with_an_invalid_encoding_is_rejected() {
    // An unknown encoding rejected the options the same way, and the source silently fell back to
    // Mapbox.
    let mut error = Error::default();
    let source = convert_source_json(
        r#"{ "type": "raster-dem", "url": "mbtiles://dem", "encoding": "terarium" }"#,
        &mut error,
        "dem",
    );
    assert!(source.is_none());
    assert_eq!(Some(0), error.message.find("invalid encoding"));
}

// gtest: SourceOptions.RasterDEMSourceWithValidOrNoOptions
#[test]
fn raster_dem_source_with_valid_or_no_options() {
    // Valid options, or none at all, still build the source.
    let mut error = Error::default();
    assert!(
        convert_source_json(
            r#"{ "type": "raster-dem", "url": "mbtiles://dem", "encoding": "terrarium", "maxzoom": 16 }"#,
            &mut error,
            "dem",
        )
        .is_some()
    );
    assert!(convert_source_json(r#"{ "type": "raster-dem", "url": "mbtiles://dem" }"#, &mut error, "dem").is_some());
}

// gtest: SourceOptions.RasterSourceZooms
#[test]
fn raster_source_zooms() {
    // A raster source takes inline zooms too, and rejects an invalid one, as vector sources do.
    let mut error = Error::default();
    assert!(
        convert_source_json(
            r#"{ "type": "raster", "url": "mbtiles://imagery", "minzoom": 2, "maxzoom": 18 }"#,
            &mut error,
            "imagery",
        )
        .is_some()
    );
    assert!(
        convert_source_json(
            r#"{ "type": "raster", "url": "mbtiles://imagery", "minzoom": "low" }"#,
            &mut error,
            "imagery"
        )
        .is_none()
    );
    assert_eq!(Some(0), error.message.find("invalid minzoom"));
}
