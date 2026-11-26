use super::util::parse_json;
use crate::conversion::tileset::convert_tileset;
use crate::conversion::{Convertible, Error};
use crate::layer::SourceKind;
use crate::source::{Source, convert_source};
use crate::tileset::{PartialTileset, RasterEncoding, VectorEncoding};

fn convert_json(json: &str, tiles: SourceKind) -> Result<PartialTileset, String> {
    convert_tileset(&Convertible::from(&parse_json(json)), tiles)
}

fn convert_source_json(json: &str, error: &mut Error, id: &str) -> Option<Source> {
    let value = parse_json(json);
    convert_source(&Convertible::from(&value), error, id, &crate::StyleRegistry::with_builtins())
}

// gtest: SourceOptions.Basic
#[test]
fn basic() {
    assert_eq!(Ok(PartialTileset::default()), convert_json("{}", SourceKind::RasterDem));
}

// gtest: SourceOptions.ErrorHandling
#[test]
fn error_handling() {
    let error = convert_json(r#"{ "encoding": "this isn't a valid encoding" }"#, SourceKind::RasterDem).unwrap_err();
    assert!(error.starts_with("invalid encoding"), "{error}");
}

// gtest: SourceOptions.TerrariumEncodingParsed
#[test]
fn terrarium_encoding_parsed() {
    let converted = convert_json(r#"{"encoding": "terrarium"}"#, SourceKind::RasterDem).unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Terrarium));
    assert!(converted.vector_encoding.is_none());
}

// gtest: SourceOptions.MapboxEncodingParsed
#[test]
fn mapbox_encoding_parsed() {
    let converted = convert_json(r#"{"encoding": "mapbox"}"#, SourceKind::RasterDem).unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Mapbox));
    assert!(converted.vector_encoding.is_none());
}

#[test]
fn custom_encoding_parsed() {
    let dem = |json| convert_json(json, SourceKind::RasterDem);
    let converted =
        dem(r#"{"encoding": "custom", "redFactor": 0, "greenFactor": 25.6, "blueFactor": 0.1, "baseShift": 3446.4}"#)
            .unwrap();
    assert_eq!(
        converted.raster_encoding,
        Some(RasterEncoding::Custom { red_factor: 0.0, green_factor: 25.6, blue_factor: 0.1, base_shift: 3446.4 })
    );
    assert!(converted.vector_encoding.is_none());

    // The style spec's defaults.
    assert_eq!(
        dem(r#"{"encoding": "custom"}"#).unwrap().raster_encoding,
        Some(RasterEncoding::Custom { red_factor: 1.0, green_factor: 1.0, blue_factor: 1.0, base_shift: 0.0 })
    );

    // The factors only apply to a custom encoding.
    assert_eq!(
        dem(r#"{"encoding": "terrarium", "redFactor": 2}"#).unwrap().raster_encoding,
        Some(RasterEncoding::Terrarium)
    );

    assert_eq!(Err("invalid baseShift".to_owned()), dem(r#"{"encoding": "custom", "baseShift": "1"}"#));
}

// gtest: SourceOptions.MVTEncodingParsed
#[test]
fn mvt_encoding_parsed() {
    let converted = convert_json(r#"{"encoding": "mvt"}"#, SourceKind::Geometry).unwrap();
    assert_eq!(converted.vector_encoding, Some(VectorEncoding::Mapbox));
    assert!(converted.raster_encoding.is_none());
}

// gtest: SourceOptions.MLTEncodingParsed
#[test]
fn mlt_encoding_parsed() {
    let converted = convert_json(r#"{"encoding": "mlt"}"#, SourceKind::Geometry).unwrap();
    assert_eq!(converted.vector_encoding, Some(VectorEncoding::MLT));
    assert!(converted.raster_encoding.is_none());
}

// Each source type has its own encodings, and a raster source has none.
#[test]
fn encodings_depend_on_the_source_type() {
    assert!(convert_json(r#"{"encoding": "mlt"}"#, SourceKind::RasterDem).is_err());
    assert!(convert_json(r#"{"encoding": "terrarium"}"#, SourceKind::Geometry).is_err());
    assert_eq!(Ok(PartialTileset::default()), convert_json(r#"{"encoding": "mlt"}"#, SourceKind::Raster));
}

// gtest: SourceOptions.ZoomRangeParsed
#[test]
fn zoom_range_parsed() {
    let converted = convert_json(r#"{ "minzoom": 5, "maxzoom": 14 }"#, SourceKind::RasterDem).unwrap();
    assert_eq!(converted.min_zoom, Some(5));
    assert_eq!(converted.max_zoom, Some(14));
    assert!(converted.raster_encoding.is_none());
    assert!(converted.vector_encoding.is_none());
}

// gtest: SourceOptions.ZoomRangeAlongsideEncoding
#[test]
fn zoom_range_alongside_encoding() {
    let converted = convert_json(r#"{ "encoding": "terrarium", "maxzoom": 15 }"#, SourceKind::RasterDem).unwrap();
    assert_eq!(converted.raster_encoding, Some(RasterEncoding::Terrarium));
    assert!(converted.min_zoom.is_none());
    assert_eq!(converted.max_zoom, Some(15));
}

// gtest: SourceOptions.InvalidMaxzoomType
#[test]
fn invalid_maxzoom_type() {
    assert_eq!(Err("invalid maxzoom".to_owned()), convert_json(r#"{ "maxzoom": "high" }"#, SourceKind::RasterDem));
}

// gtest: SourceOptions.ZoomOutOfRange
#[test]
fn zoom_out_of_range() {
    assert_eq!(Err("invalid minzoom".to_owned()), convert_json(r#"{ "minzoom": -1 }"#, SourceKind::RasterDem));
    assert_eq!(Err("invalid maxzoom".to_owned()), convert_json(r#"{ "maxzoom": 300 }"#, SourceKind::RasterDem));
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
