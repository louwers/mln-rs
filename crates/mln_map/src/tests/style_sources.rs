//! The map's sources: their handles, and the records the style holds.

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::Layer;
use mln_style::sources::tile_source::TileSource;
use mln_util::ErrorKind;
use mln_util::tileset::PartialTileset;

use crate::{Map, MapOptions};

const EMPTY_GEOJSON_SOURCE: &str = r#"{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}"#;

fn loaded_map(run_loop: &RunLoop) -> Map {
    let file_source = Arc::new(StubFileSource::new(run_loop, ResponseType::Asynchronous));
    let mut map = Map::with_file_source(file_source, MapOptions::default(), run_loop).expect("map");
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    map
}

fn vector_source(id: &str) -> Box<dyn mln_style::source::Source> {
    Box::new(TileSource::vector(id, Some("mptiler://tiles/contours".to_owned()), PartialTileset::default()))
}

fn record_ids(map: &Map) -> Vec<String> {
    map.style().get_sources().iter().map(|source| source.id.clone()).collect()
}

// gtest: Style.DuplicateSource
#[test]
fn duplicate_source() {
    let run_loop = RunLoop::new();
    let mut map = loaded_map(&run_loop);
    map.add_source(vector_source("sourceId")).unwrap();
    let error = map.add_source(vector_source("sourceId")).expect_err("a duplicate source id");
    assert_eq!(ErrorKind::AlreadyExists, error.kind);
}

// gtest: Style.RemoveSourceInUse
#[test]
fn remove_source_in_use() {
    let run_loop = RunLoop::new();
    let mut map = loaded_map(&run_loop);
    map.add_source(vector_source("sourceId")).unwrap();
    let layer = Layer::new(map.style().registry(), "line", "layerId", "sourceId").unwrap();
    map.style_mut().add_layer(layer, None).unwrap();

    assert!(map.remove_source("sourceId").is_none());
    assert!(map.source("sourceId").is_some());
    assert_eq!(["sourceId"], *record_ids(&map));
}

#[test]
fn sources_are_listed_and_recorded_in_the_order_they_were_added() {
    let run_loop = RunLoop::new();
    let mut map = loaded_map(&run_loop);
    let updates = map.update_generation();
    for id in ["c", "b", "a"] {
        map.add_source(vector_source(id)).unwrap();
    }
    assert!(map.update_generation() > updates);
    let handles: Vec<_> = map.sources().map(mln_style::source::Source::get_id).collect();
    assert_eq!(["c", "b", "a"], *handles);
    assert_eq!(["c", "b", "a"], *record_ids(&map));

    assert!(map.remove_source("b").is_some());
    assert_eq!(["c", "a"], *record_ids(&map));
    assert!(map.source("b").is_none());
}

#[test]
fn a_source_id_is_taken_once() {
    let run_loop = RunLoop::new();
    let mut map = loaded_map(&run_loop);
    map.add_source_json("owned-source", EMPTY_GEOJSON_SOURCE).unwrap();
    assert_eq!(map.source("owned-source").unwrap().get_id(), "owned-source");
    let error = map.add_source_json("owned-source", EMPTY_GEOJSON_SOURCE).expect_err("a taken id");
    assert_eq!(ErrorKind::AlreadyExists, error.kind);
    assert_eq!(r#"source "owned-source" already exists"#, error.message);
}

#[test]
fn malformed_sources_are_invalid_arguments() {
    let run_loop = RunLoop::new();
    let mut map = loaded_map(&run_loop);
    let error = map.add_source_json("unknown-type", r#"{"type":"no-such-source-type"}"#).unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind);
    assert!(error.message.starts_with(r#"style source "unknown-type": "#), "{}", error.message);
    let error = map.add_source_json("truncated", r#"{"type":"#).unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, error.kind);
    assert!(error.message.starts_with(r#"style source "truncated" is not valid JSON"#), "{}", error.message);
    assert_eq!(0, map.sources().count());
}

#[test]
fn a_source_json_needs_a_loaded_style() {
    let run_loop = RunLoop::new();
    let file_source = Arc::new(StubFileSource::new(&run_loop, ResponseType::Asynchronous));
    let mut map = Map::with_file_source(file_source, MapOptions::default(), &run_loop).expect("map");
    let error = map.add_source_json("points", EMPTY_GEOJSON_SOURCE).unwrap_err();
    assert_eq!(ErrorKind::InvalidState, error.kind);
    assert_eq!("style is not loaded", error.message);
}
