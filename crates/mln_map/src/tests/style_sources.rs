//! The map's sources, and the loads of their descriptions.

use std::sync::Arc;

use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Response};
use mln_style::source::SourceData;
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceType, geojson_data, geojson_source};
use mln_style::sources::{ImageSourceType, VectorSourceType, image_source, vector_source as tileset_vector_source};
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Edit, Layer};
use mln_util::ErrorKind;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::tileset::PartialTileset;

use crate::{Map, MapEvent, MapOptions};

const EMPTY_GEOJSON_SOURCE: &str = r#"{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}"#;

fn loaded_map(run_loop: &Queue<()>) -> Map {
    let file_source = StubFileSource::on_queue(run_loop, ResponseType::Asynchronous);
    let mut map = Map::with_file_source(file_source, MapOptions::default(), &Pool::local()).expect("map");
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    map
}

fn vector_source(id: &str) -> mln_style::Source {
    tileset_vector_source(id, Some("mptiler://tiles/contours".to_owned()), PartialTileset::default())
}

fn record_ids(map: &Map) -> Vec<String> {
    map.style().get_sources().iter().map(|source| source.id().to_owned()).collect()
}

// gtest: Style.DuplicateSource
#[test]
fn duplicate_source() {
    let run_loop = Queue::<()>::new();
    let mut map = loaded_map(&run_loop);
    map.add_source(vector_source("sourceId")).unwrap();
    let error = map.add_source(vector_source("sourceId")).expect_err("a duplicate source id");
    assert_eq!(ErrorKind::AlreadyExists, error.kind);
}

// gtest: Style.RemoveSourceInUse
#[test]
fn remove_source_in_use() {
    let run_loop = Queue::<()>::new();
    let mut map = loaded_map(&run_loop);
    map.add_source(vector_source("sourceId")).unwrap();
    let layer = Layer::new(map.style().registry(), "line", "layerId", "sourceId").unwrap();
    map.edit_style(|style| style.apply(Edit::AddLayer { layer, before: None })).unwrap();

    assert!(map.remove_source("sourceId").is_none());
    assert!(map.source("sourceId").is_some());
    assert_eq!(["sourceId"], *record_ids(&map));
}

#[test]
fn sources_are_listed_in_the_order_they_were_added() {
    let run_loop = Queue::<()>::new();
    let mut map = loaded_map(&run_loop);
    let updates = map.update_generation();
    for id in ["c", "b", "a"] {
        map.add_source(vector_source(id)).unwrap();
    }
    assert!(map.update_generation() > updates);
    let listed: Vec<_> = map.sources().map(mln_style::Source::id).collect();
    assert_eq!(["c", "b", "a"], *listed);
    assert_eq!(["c", "b", "a"], *record_ids(&map));

    assert!(map.remove_source("b").is_some());
    assert_eq!(["c", "a"], *record_ids(&map));
    assert!(map.source("b").is_none());
}

#[test]
fn a_source_id_is_taken_once() {
    let run_loop = Queue::<()>::new();
    let mut map = loaded_map(&run_loop);
    map.add_source_json("owned-source", EMPTY_GEOJSON_SOURCE).unwrap();
    assert_eq!(map.source("owned-source").unwrap().id(), "owned-source");
    let error = map.add_source_json("owned-source", EMPTY_GEOJSON_SOURCE).expect_err("a taken id");
    assert_eq!(ErrorKind::AlreadyExists, error.kind);
    assert_eq!(r#"source "owned-source" already exists"#, error.message);
}

#[test]
fn malformed_sources_are_invalid_arguments() {
    let run_loop = Queue::<()>::new();
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
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    let mut map = Map::with_file_source(file_source, MapOptions::default(), &Pool::local()).expect("map");
    let error = map.add_source_json("points", EMPTY_GEOJSON_SOURCE).unwrap_err();
    assert_eq!(ErrorKind::InvalidState, error.kind);
    assert_eq!("style is not loaded", error.message);
}

// MARK: - Description loads

/// A map over `file_source` whose style has no sources, and whose
/// background work runs when the test runs its pool.
fn map_over(file_source: &Arc<StubFileSource>) -> Map {
    let file_source: Arc<dyn mln_storage::FileSource> = Arc::<StubFileSource>::clone(file_source);
    let mut map = Map::with_file_source(file_source, MapOptions::default(), &Pool::local()).expect("map");
    map.load_style_json(r#"{"version":8,"sources":{},"layers":[]}"#);
    map
}

/// Runs `map`'s pool and `run_loop`, and handles `map`'s reports, until
/// `done` holds.
fn run_until(map: &mut Map, run_loop: &Queue<()>, mut done: impl FnMut(&Map) -> bool) {
    let deadline = mln_util::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        map.process_events();
        if done(map) {
            return;
        }
        assert!(mln_util::time::Instant::now() < deadline, "timed out");
        map.pool().run_all();
        if !run_loop.run_once(&mut ()) {
            std::thread::yield_now();
        }
    }
}

fn data_response(data: &[u8]) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(data.to_vec()));
    response
}

/// A `FeatureCollection` of `count` points at 0, 0.
fn geojson_points(count: usize) -> Vec<u8> {
    let feature = r#"{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [0, 0]}}"#;
    let features = vec![feature; count].join(",");
    format!(r#"{{"type": "FeatureCollection", "features": [{features}]}}"#).into_bytes()
}

/// The number of features in tile 0/0/0 of the data of `map`'s GeoJSON
/// source `id`.
fn feature_count(map: &Map, id: &str) -> usize {
    let Some(data) = map.source(id).and_then(geojson_data) else { return 0 };
    data.get_tile(&CanonicalTileID::new(0, 0, 0)).len()
}

fn set_geojson_url(map: &mut Map, id: &str, url: &str) {
    let edit = GeoJsonSourceType::set_url(id, url);
    map.edit_style(|style| style.apply(edit)).unwrap();
}

fn source_changes(events: &[MapEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            MapEvent::SourceChanged { source_id } => Some(source_id.as_str()),
            _ => None,
        })
        .collect()
}

// gtest: Source.LoadingFail
#[test]
fn loading_fail() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("url", resource.url);
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
        Some(response)
    });
    let mut map = map_over(&file_source);
    map.add_source(tileset_vector_source("source", Some("url".to_owned()), PartialTileset::default())).unwrap();
    run_until(&mut map, &run_loop, |map| map.last_style_error().is_some());
    assert_eq!("Failed by the test case", map.last_style_error().unwrap().to_string());
    assert!(!map.style_loaded());
}

// A TileJSON's tileset reaches the style, which reports the source changed.
#[test]
fn a_tilejson_describes_its_source() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |_| Some(data_response(br#"{"tiles": ["tiles"], "attribution": "a"}"#)));
    let mut map = map_over(&file_source);
    map.add_source(tileset_vector_source("source", Some("url".to_owned()), PartialTileset::default())).unwrap();
    assert!(!map.style_loaded());
    run_until(&mut map, &run_loop, Map::style_loaded);
    assert_eq!(Some("a".to_owned()), map.source("source").unwrap().data().attribution());
    assert_eq!(["source"], *source_changes(&map.drain_events()));
}

// A tileset that the style lists is the source's description from the first
// version that holds it.
#[test]
fn an_inline_tileset_describes_its_source_at_once() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    let mut map = map_over(&file_source);
    let tileset = PartialTileset { tiles: Some(vec!["tiles".to_owned()]), ..PartialTileset::default() };
    map.add_source(tileset_vector_source("source", None, tileset)).unwrap();
    let data = SourceData::<VectorSourceType>::recover(map.source("source").unwrap()).unwrap();
    assert_eq!(vec!["tiles".to_owned()], data.description.as_ref().expect("a tileset").tiles);
    assert!(map.style_loaded());
}

// gtest: Source.GeoJSonSourceUrlUpdate
#[test]
fn geojson_source_url_update() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Source, |resource| {
        assert_eq!("http://source-url.ext", resource.url);
        Some(data_response(
            br#"{"geometry": {"type": "Point", "coordinates": [1.1, 1.1]}, "type": "Feature", "properties": {}}"#,
        ))
    });
    let mut map = map_over(&file_source);
    map.add_source(geojson_source("source", GeoJSONOptions::default_options())).unwrap();
    assert!(map.style_loaded());
    set_geojson_url(&mut map, "source", "http://source-url.ext");
    assert!(!map.style_loaded());
    run_until(&mut map, &run_loop, Map::style_loaded);
    assert!(map.source("source").and_then(geojson_data).is_some());
}

// gtest: Source.GeoJSONSourceDataReplacesBuildingURLData
#[test]
fn geojson_source_data_replaces_building_url_data() {
    let run_loop = Queue::<()>::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    file_source.set_response(Kind::Source, |_| Some(data_response(&geojson_points(1))));
    let mut map = map_over(&file_source);
    map.add_source(geojson_source("source", GeoJSONOptions::default_options())).unwrap();
    set_geojson_url(&mut map, "source", "url");
    // The URL's data arrives and starts building in the background.
    file_source.respond_to_all();
    map.process_events();

    let source = map.source("source").unwrap();
    let builder = SourceData::<GeoJsonSourceType>::recover(source).unwrap().options.data_builder();
    let data = builder.build(serde_json::from_slice(&geojson_points(2)).unwrap()).unwrap();
    let edit = GeoJsonSourceType::set_data("source", Some(data));
    map.edit_style(|style| style.apply(edit)).unwrap();
    assert!(map.style_loaded());

    // The build's reply finds the source moved on.
    map.pool().run_all();
    run_until(&mut map, &run_loop, |map| !map.has_reports());
    assert_eq!(2, feature_count(&map, "source"));
}

// gtest: Source.GeoJSONSourceURLReplacesBuildingURLData
#[test]
fn geojson_source_url_replaces_building_url_data() {
    let run_loop = Queue::<()>::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    file_source.set_response(Kind::Source, |resource| {
        Some(data_response(&geojson_points(if resource.url == "old" { 1 } else { 2 })))
    });
    let mut map = map_over(&file_source);
    map.add_source(geojson_source("source", GeoJSONOptions::default_options())).unwrap();
    set_geojson_url(&mut map, "source", "old");
    // The old URL's data arrives and starts building in the background.
    file_source.respond_to_all();
    map.process_events();

    set_geojson_url(&mut map, "source", "new");
    map.pool().run_all();
    run_until(&mut map, &run_loop, |map| !map.has_reports());
    assert!(!map.style_loaded());
    assert_eq!(0, feature_count(&map, "source"));

    file_source.respond_to_all();
    run_until(&mut map, &run_loop, Map::style_loaded);
    assert_eq!(2, feature_count(&map, "source"));
}

// A description that builds while the source changes keeps the change.
#[test]
fn a_geojson_build_keeps_the_synchronous_update_override() {
    let run_loop = Queue::<()>::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    file_source.set_response(Kind::Source, |_| Some(data_response(&geojson_points(1))));
    let mut map = map_over(&file_source);
    let options = GeoJSONOptions { synchronous_update: true, ..Default::default() };
    map.add_source(geojson_source("source", Arc::new(options))).unwrap();
    set_geojson_url(&mut map, "source", "url");
    file_source.respond_to_all();
    map.process_events();
    map.drain_events();

    let edit = GeoJsonSourceType::set_synchronous("source", false);
    map.edit_style(|style| style.apply(edit)).unwrap();
    assert_eq!(["source"], *source_changes(&map.drain_events()));

    run_until(&mut map, &run_loop, Map::style_loaded);
    assert_eq!(1, feature_count(&map, "source"));
    assert!(!map.source("source").unwrap().data().is_update_synchronous());
}

// gtest: Source.ImageSourceImageUpdate
#[test]
fn image_source_image_update() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_global_response(|resource| {
        assert_eq!("http://url", resource.url);
        let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/test/fixtures/image/no_profile.png");
        Some(data_response(&std::fs::read(path).unwrap()))
    });
    let mut map = map_over(&file_source);
    map.add_source(image_source("source", [LatLng::default(); 4])).unwrap();
    let edit = ImageSourceType::set_url("source", "http://url");
    map.edit_style(|style| style.apply(edit)).unwrap();
    run_until(&mut map, &run_loop, Map::style_loaded);
    let image =
        |map: &Map| SourceData::<ImageSourceType>::recover(map.source("source").unwrap()).unwrap().description.clone();
    assert!(image(&map).is_some());
    map.drain_events();

    let mut rgba = PremultipliedImage::new(Size::new(2, 1));
    rgba.data.copy_from_slice(&[255, 254, 253, 0, 255, 254, 253, 0]);
    let edit = ImageSourceType::set_image("source", rgba);
    map.edit_style(|style| style.apply(edit)).unwrap();
    assert_eq!(["source"], *source_changes(&map.drain_events()));
    assert_eq!(Size::new(2, 1), image(&map).unwrap().size);
    assert!(map.style_loaded());
}
