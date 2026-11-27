//! Source types' style halves: how their sources convert from style JSON,
//! combine with a TileJSON and change through edits. How their descriptions
//! load is their engine halves' (`mln_builtins`' tests).

use std::sync::Arc;

use super::util::parse_json;
use crate::conversion::{Convertible, Error};
use crate::layer::SourceKind;
use crate::source::{Source, SourceData, convert_source};
use crate::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, geojson_source};
use crate::sources::tileset::{TilesetSourceType, VectorKind, raster_dem_source, raster_source, vector_source};
use crate::tileset::{PartialTileset, RasterEncoding, Tileset, VectorEncoding};
use crate::{Edit, Style, StyleRegistry};

/// The tileset that `source`, a tileset source of `tiles`, takes from a
/// TileJSON with `"minzoom": 4, "maxzoom": 12`.
fn tileset_with_tilejson<K: crate::sources::tileset::TilesetKind>(source: &Source) -> Tileset {
    let data = SourceData::<TilesetSourceType<K>>::recover(source).expect("a tileset source of the kind");
    let tilejson = br#"{ "tilejson": "2.1.0", "tiles": [ "tiles" ], "minzoom": 4, "maxzoom": 12 }"#;
    data.options.tileset_with_tilejson(tilejson, K::INFO.layer_kind).unwrap_or_else(|error| panic!("{error}"))
}

fn style_with(source: Source) -> Style {
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.apply(Edit::AddSource(source)).unwrap();
    style
}

// gtest: Source.LoadingCorrupt
#[test]
fn a_corrupt_tilejson_gives_no_tileset() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(&source).unwrap();
    // C++ expects rapidjson's "Invalid value. at offset 0"; JSON syntax
    // errors carry serde_json's message in the port.
    let error = data.options.tileset_with_tilejson(b"CORRUPTED", SourceKind::Geometry).expect_err("an error");
    assert_eq!("expected value at line 1 column 1", error);
}

// gtest: Source.GeoJSONSourceSynchronousUpdateOverrideReplacesTheOption
#[test]
fn geojson_source_synchronous_update_override_replaces_the_option() {
    let options = GeoJSONOptions { synchronous_update: true, ..Default::default() };
    let mut style = style_with(geojson_source("source", Arc::new(options)));
    let synchronous = |source: &Source| {
        SourceData::<GeoJsonSourceType>::recover(source).expect("a GeoJSON source").options.synchronous_update
    };
    let synchronous_in = |style: &Style, id: &str| synchronous(style.get_source(id).unwrap());
    assert!(synchronous_in(&style, "source"));

    let published = style.get_source("source").unwrap().clone();
    style.apply(GeoJsonSourceType::set_synchronous("source", false)).unwrap();
    assert!(!synchronous_in(&style, "source"));
    // The version the renderer may hold keeps its mode.
    assert!(synchronous(&published));

    // Setting the mode the source has changes nothing.
    let before = style.get_source("source").unwrap().clone();
    style.apply(GeoJsonSourceType::set_synchronous("source", false)).unwrap();
    assert!(Source::ptr_eq(&before, style.get_source("source").unwrap()));

    style.apply(GeoJsonSourceType::set_synchronous("source", true)).unwrap();
    assert!(synchronous_in(&style, "source"));

    style.apply(Edit::AddSource(geojson_source("asynchronous", GeoJSONOptions::default_options()))).unwrap();
    assert!(!synchronous_in(&style, "asynchronous"));
    style.apply(GeoJsonSourceType::set_synchronous("asynchronous", true)).unwrap();
    assert!(synchronous_in(&style, "asynchronous"));
}

// The style holds a source's description as its loader gives it, whatever
// its type.
#[test]
fn a_description_gives_the_source_it() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let mut style = style_with(source.clone());
    let edit = Edit::SetSourceDescription {
        id: "source".to_owned(),
        instance: source.instance(),
        load: 0,
        description: Some(Arc::new(7_u32)),
    };
    style.apply(edit).unwrap();
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(style.get_source("source").unwrap()).unwrap();
    assert_eq!(Some(&7), data.description::<u32>());
    assert_eq!(None, data.description::<Tileset>(), "a description of another type");
}

#[test]
fn a_description_of_a_load_the_source_moved_on_from_changes_nothing() {
    let source = geojson_source("source", GeoJSONOptions::default_options());
    let mut style = style_with(source.clone());
    style.apply(GeoJsonSourceType::set_url("source", "url")).unwrap();
    let before = style.clone();
    let edit = Edit::SetSourceDescription {
        id: "source".to_owned(),
        instance: source.instance(),
        load: 0,
        description: Some(Arc::new(())),
    };
    style.apply(edit).unwrap();
    assert!(before.is_same_version(&style));
}

// gtest: Source.RasterDEMInlineZoomOverridesTileJSON
#[test]
fn raster_dem_inline_zoom_overrides_tile_json() {
    let source = raster_dem_source(
        "source",
        Some("url".to_owned()),
        PartialTileset { raster_encoding: Some(RasterEncoding::Terrarium), max_zoom: Some(15), ..Default::default() },
        512,
    );
    let tileset = tileset_with_tilejson::<crate::sources::tileset::RasterDemKind>(&source);
    // The inline maxzoom wins over the TileJSON's; minzoom was not given inline, so the TileJSON
    // value is kept.
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(15, tileset.zoom_range.max);
    assert_eq!(Some(RasterEncoding::Terrarium), tileset.raster_encoding);
}

// gtest: Source.RasterInlineZoomOverridesTileJSON
#[test]
fn raster_inline_zoom_overrides_tile_json() {
    let source = raster_source(
        "source",
        Some("url".to_owned()),
        PartialTileset { max_zoom: Some(16), ..Default::default() },
        512,
    );
    let tileset = tileset_with_tilejson::<crate::sources::tileset::RasterKind>(&source);
    // The inline maxzoom wins; minzoom was not given inline, so the TileJSON's is kept.
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(16, tileset.zoom_range.max);
}

// gtest: Source.RasterDEMTileJSONZoomKeptWithoutInlineOverride
#[test]
fn raster_dem_tile_json_zoom_kept_without_inline_override() {
    let source = raster_dem_source("source", Some("url".to_owned()), PartialTileset::default(), 512);
    let tileset = tileset_with_tilejson::<crate::sources::tileset::RasterDemKind>(&source);
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(12, tileset.zoom_range.max);
}

/// The source the style JSON `json` converts to.
fn converted_source(json: &str) -> Result<Source, String> {
    let mut error = Error::default();
    convert_source(&Convertible::from(&parse_json(json)), &mut error, "source", &StyleRegistry::with_builtins())
        .ok_or(error.message)
}

// A vector source's tileset takes the style's sparse, over a TileJSON or inline.
#[test]
fn vector_sources_take_the_style_sparse() {
    let source =
        converted_source(r#"{"type": "vector", "url": "url", "sparse": true}"#).unwrap_or_else(|e| panic!("{e}"));
    assert!(tileset_with_tilejson::<VectorKind>(&source).sparse);

    let source = converted_source(r#"{"type": "vector", "url": "url"}"#).unwrap();
    assert!(!tileset_with_tilejson::<VectorKind>(&source).sparse);

    let source = converted_source(r#"{"type": "vector", "tiles": ["tiles"], "sparse": true}"#).unwrap();
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(&source).unwrap();
    assert!(data.options.inline_tileset().unwrap().sparse);

    let Err(error) = converted_source(r#"{"type": "vector", "url": "url", "sparse": "yes"}"#) else {
        panic!("an invalid sparse converts");
    };
    assert!(error.contains("sparse"), "{error}");
}

// A vector source's style encoding applies to the tileset from its TileJSON.
#[test]
fn vector_source_style_encoding_applies_to_tile_json() {
    let source = converted_source(r#"{"type": "vector", "url": "url", "encoding": "mlt"}"#).unwrap();
    assert_eq!(Some(VectorEncoding::MLT), tileset_with_tilejson::<VectorKind>(&source).vector_encoding);
}

// GeoJSON that the style gives converts with the style, and whatever it
// holds fails only the source, as it loads.
#[test]
fn inline_geojson_converts_without_indexing() {
    let options = |json: &str| {
        let source = converted_source(json).unwrap_or_else(|e| panic!("{e}"));
        SourceData::<GeoJsonSourceType>::recover(&source).unwrap().options.clone()
    };
    let point = options(r#"{"type": "geojson", "data": {"type": "Point", "coordinates": [1, 2]}}"#);
    assert!(matches!(point.inline.as_deref(), Some(Ok(_))));
    assert_eq!(None, point.url);

    let not_geojson = options(r#"{"type": "geojson", "data": {"type": "Nothing"}}"#);
    assert!(matches!(not_geojson.inline.as_deref(), Some(Err(_))));

    // A clustered line is GeoJSON, which fails to cluster as it loads.
    let clustered_line = options(
        r#"{"type": "geojson", "cluster": true, "data": {"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {}, "geometry": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}}
        ]}}"#,
    );
    assert!(matches!(clustered_line.inline.as_deref(), Some(Ok(_))));
}
