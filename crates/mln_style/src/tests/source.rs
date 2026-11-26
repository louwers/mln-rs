//! Source types: how their sources convert from style JSON, describe their
//! fetched data and change through edits. The map's tests cover the loads
//! themselves.

use std::sync::Arc;

use super::util::parse_json;
use crate::conversion::{Convertible, Error};
use crate::source::{Described, ErasedDescription, Fetch, Load, Source, SourceData, convert_source};
use crate::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, geojson_source};
use crate::sources::tileset::{
    RasterDemKind, RasterKind, TilesetKind, TilesetSourceType, VectorKind, raster_dem_source, raster_source,
    vector_source,
};
use crate::tileset::{PartialTileset, RasterEncoding, Tileset, VectorEncoding};
use crate::{Edit, Style, StyleRegistry};

/// What `source`'s type describes `data`, its fetched resource, as.
fn describe(source: &Source, data: &[u8]) -> Described<ErasedDescription> {
    let (sender, receiver) = std::sync::mpsc::channel();
    source.data().describe_job(Some(Arc::new(data.to_vec())))(Box::new(move |described| {
        let _ = sender.send(described);
    }));
    receiver.recv().expect("an answer")
}

/// `source` with the description `data` gives it.
fn described(source: &Source, data: &[u8]) -> Source {
    let description = describe(source, data).unwrap_or_else(|error| panic!("{error}"));
    source.with_description(description).expect("a description of the source's type")
}

/// `source` with the description that its type knows without a fetch.
fn ready(source: &Source) -> Source {
    let Load::Ready(description) = source.data().plan_load() else { panic!("a description to fetch") };
    source.with_description(Some(description)).expect("a description of the source's type")
}

/// The loaded tileset of `source`, a tileset source of the kind `K`.
fn loaded_tileset<K: TilesetKind>(source: &Source) -> Option<Tileset> {
    SourceData::<TilesetSourceType<K>>::recover(source).expect("a tileset source of the kind").description.clone()
}

/// The tileset `source`, a tileset source of the kind `K`, loads from a
/// TileJSON with `"minzoom": 4, "maxzoom": 12`.
fn tileset_loaded_from_tilejson<K: TilesetKind>(source: &Source) -> Tileset {
    let source = described(source, br#"{ "tilejson": "2.1.0", "tiles": [ "tiles" ], "minzoom": 4, "maxzoom": 12 }"#);
    loaded_tileset::<K>(&source).expect("a tileset")
}

fn style_with(source: Source) -> Style {
    let mut style = Style::new(StyleRegistry::with_builtins());
    style.apply(Edit::AddSource(source)).unwrap();
    style
}

// gtest: Source.LoadingCorrupt
#[test]
fn loading_corrupt() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let Load::Fetch(fetch) = source.data().plan_load() else { panic!("a TileJSON to fetch") };
    assert_eq!(Fetch::Source("url".to_owned()), fetch);
    // C++ expects rapidjson's "Invalid value. at offset 0"; JSON syntax
    // errors carry serde_json's message in the port.
    let error = describe(&source, b"CORRUPTED").expect_err("an error");
    assert_eq!("expected value at line 1 column 1", error.to_string());
}

// gtest: Source.GeoJSONSourceSynchronousUpdateOverrideReplacesTheOption
#[test]
fn geojson_source_synchronous_update_override_replaces_the_option() {
    let options = GeoJSONOptions { synchronous_update: true, ..Default::default() };
    let mut style = style_with(geojson_source("source", Arc::new(options)));
    let synchronous = |style: &Style, id: &str| style.get_source(id).unwrap().data().is_update_synchronous();
    assert!(synchronous(&style, "source"));

    let published = style.get_source("source").unwrap().clone();
    style.apply(GeoJsonSourceType::set_synchronous("source", false)).unwrap();
    assert!(!synchronous(&style, "source"));
    // The version the renderer may hold keeps its mode.
    assert!(published.data().is_update_synchronous());

    // Setting the mode the source has changes nothing.
    let before = style.get_source("source").unwrap().clone();
    style.apply(GeoJsonSourceType::set_synchronous("source", false)).unwrap();
    assert!(Source::ptr_eq(&before, style.get_source("source").unwrap()));

    style.apply(GeoJsonSourceType::set_synchronous("source", true)).unwrap();
    assert!(synchronous(&style, "source"));

    style.apply(Edit::AddSource(geojson_source("asynchronous", GeoJSONOptions::default_options()))).unwrap();
    assert!(!synchronous(&style, "asynchronous"));
    style.apply(GeoJsonSourceType::set_synchronous("asynchronous", true)).unwrap();
    assert!(synchronous(&style, "asynchronous"));
}

// gtest: Source.GeoJSONSourceKeepsTheStyleAttribution
#[test]
fn geojson_source_keeps_the_style_attribution() {
    let source = converted_source(
        r#"{"type": "geojson", "attribution": "Fixture data", "data": {"type": "FeatureCollection", "features": []}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(Some("Fixture data".to_owned()), source.data().attribution());

    let mut style = style_with(source);
    let options = crate::sources::geojson::GeoJsonSourceOptions::new(GeoJSONOptions::default_options());
    let data = options.data_builder().build(r#"{"type": "Point", "coordinates": [1.1, 1.1]}"#.parse().unwrap());
    style.apply(GeoJsonSourceType::set_data("source", Some(data.unwrap()))).unwrap();
    assert_eq!(Some("Fixture data".to_owned()), style.get_source("source").unwrap().data().attribution());

    style.apply(GeoJsonSourceType::set_url("source", "url")).unwrap();
    let source = described(style.get_source("source").unwrap(), br#"{"type": "Point", "coordinates": [2.2, 2.2]}"#);
    assert!(crate::sources::geojson::geojson_data(&source).is_some());
    assert_eq!(Some("Fixture data".to_owned()), source.data().attribution());
}

#[test]
fn geojson_that_does_not_parse_describes_no_data() {
    let source = geojson_source("source", GeoJSONOptions::default_options());
    assert!(matches!(describe(&source, b"not json"), Ok(None)));
}

// A vector source's TileJSON gives it its tileset, which changes its metadata.
#[test]
fn a_tilejson_describes_a_vector_source() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let mut style = style_with(source.clone());
    let instance = source.instance();
    let description = describe(&source, br#"{"tiles": ["tiles"]}"#);
    let edit = Edit::SetSourceDescription { id: "source".to_owned(), instance, load: 0, result: description };
    style.apply(edit).unwrap();
    let source_now = style.get_source("source").unwrap();
    assert_eq!(Some(vec!["tiles".to_owned()]), loaded_tileset::<VectorKind>(source_now).map(|tileset| tileset.tiles));
    assert!(source_now.metadata_changed_from(&source));
}

#[test]
fn a_description_of_a_load_the_source_moved_on_from_changes_nothing() {
    let source = geojson_source("source", GeoJSONOptions::default_options());
    let mut style = style_with(source.clone());
    style.apply(GeoJsonSourceType::set_url("source", "url")).unwrap();
    let before = style.clone();
    let description = describe(&source, br#"{"type": "Point", "coordinates": [2.2, 2.2]}"#);
    let edit = Edit::SetSourceDescription {
        id: "source".to_owned(),
        instance: source.instance(),
        load: 0,
        result: description,
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
    let tileset = tileset_loaded_from_tilejson::<RasterDemKind>(&source);
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
    let tileset = tileset_loaded_from_tilejson::<RasterKind>(&source);
    // The inline maxzoom wins; minzoom was not given inline, so the TileJSON's is kept.
    assert_eq!(4, tileset.zoom_range.min);
    assert_eq!(16, tileset.zoom_range.max);
}

// gtest: Source.RasterDEMTileJSONZoomKeptWithoutInlineOverride
#[test]
fn raster_dem_tile_json_zoom_kept_without_inline_override() {
    let source = raster_dem_source("source", Some("url".to_owned()), PartialTileset::default(), 512);
    let tileset = tileset_loaded_from_tilejson::<RasterDemKind>(&source);
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
    assert!(tileset_loaded_from_tilejson::<VectorKind>(&source).sparse);

    let source = converted_source(r#"{"type": "vector", "url": "url"}"#).unwrap();
    assert!(!tileset_loaded_from_tilejson::<VectorKind>(&source).sparse);

    let source = ready(&converted_source(r#"{"type": "vector", "tiles": ["tiles"], "sparse": true}"#).unwrap());
    assert!(loaded_tileset::<VectorKind>(&source).unwrap().sparse);

    let Err(error) = converted_source(r#"{"type": "vector", "url": "url", "sparse": "yes"}"#) else {
        panic!("an invalid sparse converts");
    };
    assert!(error.contains("sparse"), "{error}");
}

// A vector source's style encoding applies to the tileset from its TileJSON.
#[test]
fn vector_source_style_encoding_applies_to_tile_json() {
    let source = converted_source(r#"{"type": "vector", "url": "url", "encoding": "mlt"}"#).unwrap();
    assert_eq!(Some(VectorEncoding::MLT), tileset_loaded_from_tilejson::<VectorKind>(&source).vector_encoding);
}
