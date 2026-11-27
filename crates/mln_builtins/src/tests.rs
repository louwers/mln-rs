//! The registrations, and how the source types' sources load: what they
//! describe their fetched data and the style's GeoJSON as, and what they
//! report. `mln_headless`'s integration tests (`tests/source_tiles`) cover how
//! their tiles load.

use std::sync::Arc;

use mln_engine::RegistryBuilder;
use mln_engine::source::{Described, Fetch, Load, SourceType};
use mln_style::conversion::{Convertible, Error};
use mln_style::expression::functions::RegisterError;
use mln_style::source::{SourceData, convert_source};
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, geojson_source};
use mln_style::sources::tileset::{TilesetSourceType, VectorKind, vector_source};
use mln_style::tileset::{PartialTileset, Tileset};
use mln_style::{Edit, Source, Style};

use crate::sources::geojson::{GeoJSONData, create_geojson_data, geojson_data, set_data};
use crate::sources::{GeoJsonSource, VectorSource};
use crate::{register_builtins, registry};

/// What the source type `T` describes `data` as, for a source of `options`:
/// a fetched resource, or `None` for what it makes of the options alone.
fn describe<T: SourceType>(
    options: &mln_engine::source::OptionsOf<T>,
    data: Option<&[u8]>,
) -> Described<T::Description> {
    let (sender, receiver) = std::sync::mpsc::channel();
    T::describe(
        options,
        data.map(|data| Arc::new(data.to_vec())),
        Box::new(move |described| {
            let _ = sender.send(described);
        }),
    );
    receiver.recv().expect("an answer")
}

/// The options of `source`, a GeoJSON source.
fn geojson_options(source: &Source) -> &mln_style::sources::geojson::GeoJsonSourceOptions {
    &SourceData::<GeoJsonSourceType>::recover(source).expect("a GeoJSON source").options
}

/// `source`, a GeoJSON source, with the description `data` gives it.
fn described_geojson(source: &Source, data: Option<&[u8]>) -> Source {
    let description = describe::<GeoJsonSource>(geojson_options(source), data).unwrap_or_else(|e| panic!("{e}"));
    source.with_description(description.map(|data| -> mln_style::source::ErasedDescription { Arc::new(data) }))
}

fn style_with(source: Source) -> Style {
    let mut style = Style::new(Arc::clone(registry().style()));
    style.apply(Edit::AddSource(source)).unwrap();
    style
}

/// The source the style JSON `json` converts to.
fn converted_source(json: &str) -> Result<Source, String> {
    let value: serde_json::Value = serde_json::from_str(json).expect("JSON");
    let mut error = Error::default();
    convert_source(&Convertible::from(&value), &mut error, "source", registry().style()).ok_or(error.message)
}

#[test]
fn builtins_register_once() {
    let mut builder = RegistryBuilder::new();
    register_builtins(&mut builder).unwrap();
    assert!(matches!(register_builtins(&mut builder), Err(RegisterError::Duplicate(_))));
}

#[test]
fn builtins_register_five_source_types() {
    let names: Vec<_> = registry().style().source_types().map(|source_type| source_type.info().name).collect();
    assert_eq!(names, ["geojson", "image", "raster", "raster-dem", "vector"]);
}

#[test]
fn a_source_type_registers_once() {
    let mut builder = RegistryBuilder::new();
    builder.add_source_type::<GeoJsonSource>().unwrap();
    assert_eq!(builder.add_source_type::<GeoJsonSource>(), Err(RegisterError::Duplicate("geojson".to_owned())));
}

// gtest: Source.LoadingCorrupt
#[test]
fn loading_corrupt() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(&source).unwrap();
    let Load::Fetch(fetch) = VectorSource::load(data) else { panic!("a TileJSON to fetch") };
    assert_eq!(Fetch::Source("url".to_owned()), fetch);
    // C++ expects rapidjson's "Invalid value. at offset 0"; JSON syntax
    // errors carry serde_json's message in the port.
    let error = describe::<VectorSource>(&data.options, Some(b"CORRUPTED")).expect_err("an error");
    assert_eq!("expected value at line 1 column 1", error.to_string());
}

// A vector source's TileJSON gives it its tileset, which changes its metadata.
#[test]
fn a_tilejson_describes_a_vector_source() {
    let source = vector_source("source", Some("url".to_owned()), PartialTileset::default());
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(&source).unwrap();
    let tileset = describe::<VectorSource>(&data.options, Some(br#"{"tiles": ["tiles"], "attribution": "a"}"#))
        .unwrap()
        .expect("a tileset");
    assert_eq!(vec!["tiles".to_owned()], tileset.tiles);
    assert!(VectorSource::metadata_changed(None, Some(&tileset)));
    assert!(!VectorSource::metadata_changed(Some(&tileset), Some(&tileset.clone())));

    let source = source.with_description(Some(Arc::new(tileset)));
    assert_eq!(Some("a".to_owned()), registry().source_attribution(&source));
}

// An inline tileset is the source's description, known without a fetch.
#[test]
fn an_inline_tileset_is_ready() {
    let source = converted_source(r#"{"type": "vector", "tiles": ["tiles"], "sparse": true}"#).unwrap();
    let data = SourceData::<TilesetSourceType<VectorKind>>::recover(&source).unwrap();
    let Load::Ready(tileset) = VectorSource::load(data) else { panic!("an inline tileset") };
    assert!(tileset.sparse);
    assert_eq!(Tileset { tiles: vec!["tiles".to_owned()], sparse: true, ..Tileset::default() }.tiles, tileset.tiles);
}

// gtest: Source.GeoJSONSourceKeepsTheStyleAttribution
#[test]
fn geojson_source_keeps_the_style_attribution() {
    let source = converted_source(
        r#"{"type": "geojson", "attribution": "Fixture data", "data": {"type": "FeatureCollection", "features": []}}"#,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(Some("Fixture data".to_owned()), registry().source_attribution(&source));

    let mut style = style_with(source);
    let data = create_geojson_data(r#"{"type": "Point", "coordinates": [1.1, 1.1]}"#.parse().unwrap(), &{
        GeoJSONOptions::default_options()
    });
    style.apply(set_data("source", Some(data.unwrap()))).unwrap();
    assert_eq!(Some("Fixture data".to_owned()), registry().source_attribution(style.get_source("source").unwrap()));

    style.apply(GeoJsonSourceType::set_url("source", "url")).unwrap();
    let source = described_geojson(
        style.get_source("source").unwrap(),
        Some(br#"{"type": "Point", "coordinates": [2.2, 2.2]}"#),
    );
    assert!(geojson_data(&source).is_some());
    assert_eq!(Some("Fixture data".to_owned()), registry().source_attribution(&source));
}

#[test]
fn geojson_that_does_not_parse_describes_no_data() {
    let source = geojson_source("source", GeoJSONOptions::default_options());
    assert!(matches!(describe::<GeoJsonSource>(geojson_options(&source), Some(b"not json")), Ok(None)));
}

// The style's GeoJSON is indexed as the source loads, from its options
// alone.
#[test]
fn inline_geojson_describes_its_source() {
    let source = converted_source(r#"{"type": "geojson", "data": {"type": "Point", "coordinates": [1, 2]}}"#).unwrap();
    let data = SourceData::<GeoJsonSourceType>::recover(&source).unwrap();
    assert!(matches!(GeoJsonSource::load(data), Load::Describe));
    let source = described_geojson(&source, None);
    let features =
        geojson_data(&source).expect("the GeoJSON's data").get_tile(&mln_style::tile_id::CanonicalTileID::new(0, 0, 0));
    assert_eq!(1, features.len());
}

// GeoJSON that the style gives but that doesn't convert or index fails the
// source, which loads empty, rather than the style.
#[test]
fn invalid_inline_geojson_describes_no_data() {
    for json in [
        r#"{"type": "geojson", "data": {"type": "Nothing"}}"#,
        r#"{"type": "geojson", "cluster": true, "data": {"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {}, "geometry": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}}]}}"#,
    ] {
        let source = converted_source(json).unwrap_or_else(|e| panic!("{json}: {e}"));
        let style = style_with(source);
        let source = style.get_source("source").unwrap();
        assert!(matches!(describe::<GeoJsonSource>(geojson_options(source), None), Ok(None)), "{json}");
    }
}

// Data that the API gives replaces the style's GeoJSON, which no longer
// loads.
#[test]
fn set_data_replaces_inline_geojson() {
    let source = converted_source(r#"{"type": "geojson", "data": {"type": "Point", "coordinates": [1, 2]}}"#).unwrap();
    let mut style = style_with(source);
    let data: Arc<dyn GeoJSONData> = create_geojson_data(
        r#"{"type": "Point", "coordinates": [3, 4]}"#.parse().unwrap(),
        &GeoJSONOptions::default_options(),
    )
    .unwrap();
    style.apply(set_data("source", Some(Arc::clone(&data)))).unwrap();
    let source = style.get_source("source").unwrap();
    let options = geojson_options(source);
    assert!(options.inline.is_none() && options.url.is_none());
    assert!(matches!(GeoJsonSource::load(SourceData::<GeoJsonSourceType>::recover(source).unwrap()), Load::Done));
    assert!(Arc::ptr_eq(&data, geojson_data(source).unwrap()));
}
