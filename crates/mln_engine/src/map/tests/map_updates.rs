//! The map's update slot: the updates it publishes for its renderer, their
//! readers, and command groups.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use feature::Value;
use mln_style::error::ErrorKind;
use mln_style::feature::FeatureState;
use mln_style::size::Size;
use mln_style::source::SourceData;
use mln_style::sources::GeoJsonSourceType;
use parking_lot::Mutex;

use crate::camera::CameraOptions;
use crate::exec::queue::Queue;
use crate::map::{Map, MapOptions, TerrainOptions, TerrainSkirtLength, TileSettings};
use crate::render::update_slot::Update;
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::storage::{Kind, Response};
use crate::{RendererEvent, UpdateReader};

fn map(run_loop: &Queue<()>) -> Map {
    let options = MapOptions { size: Size::new(256, 256), ..Default::default() };
    Map::with_file_source(
        StubFileSource::on_queue(run_loop, ResponseType::Asynchronous),
        options,
        crate::tests::registry(),
        &crate::map::tests::pool(),
    )
    .expect("map")
}

/// A reader of `map`'s updates, and the times that it woke.
fn count_wakes(map: &Map) -> (UpdateReader, Arc<AtomicU64>) {
    let reader = map.update_reader();
    let wakes = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&wakes);
    reader.set_waker(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
    (reader, wakes)
}

fn assert_near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} is not {expected}");
}

#[test]
fn a_command_group_holds_its_updates_until_the_outermost_group_ends() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.drain_events();
    let reader = map.update_reader();
    let before = map.update_generation();
    let (_reader, wakes) = count_wakes(&map);

    map.begin_command_group();
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    map.begin_command_group();
    assert_eq!(map.command_group_depth(), 2);
    map.jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    map.end_command_group().expect("the inner group ends");
    map.set_tile_settings(&TileSettings { prefetch_zoom_delta: 1, ..map.snapshot().tile });
    assert_eq!(map.update_generation(), before);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    map.end_command_group().expect("the outer group ends");
    assert_eq!(map.command_group_depth(), 0);
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let update = reader.latest().expect("an update");
    assert_eq!(update.generation, before + 1);
    assert_near(update.parameters.view.zoom(), 3.0);
    assert_eq!(update.parameters.prefetch_zoom_delta, 1);

    let error = map.end_command_group().expect_err("no group is open");
    assert_eq!(error.kind, ErrorKind::InvalidState);
    assert_eq!(map.update_generation(), before + 1);
}

#[test]
fn a_hold_keeps_updates_back_apart_from_the_groups() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.drain_events();
    let before = map.update_generation();
    let (_reader, wakes) = count_wakes(&map);

    map.hold_updates();
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    assert_eq!(map.command_group_depth(), 0);
    assert_eq!(map.end_command_group().expect_err("the hold is no group").kind, ErrorKind::InvalidState);
    map.begin_command_group();
    map.jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    map.end_command_group().expect("the group ends");
    assert_eq!((map.update_generation(), map.pending_update_generation()), (before, before + 1));

    // A group open at the release keeps the update until it ends.
    map.begin_command_group();
    map.release_updates();
    assert_eq!(map.update_generation(), before);
    map.end_command_group().expect("the group ends");
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    map.hold_updates();
    map.jump_to(&CameraOptions { zoom: Some(4.0), ..Default::default() });
    map.release_updates();
    assert_eq!(map.update_generation(), before + 2);
}

#[test]
fn a_group_without_changes_publishes_nothing() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.drain_events();
    let before = map.update_generation();
    map.begin_command_group();
    map.end_command_group().expect("the group ends");
    assert_eq!(map.update_generation(), before);
}

#[test]
fn updates_carry_the_terrain_options_of_the_map_options() {
    let run_loop = Queue::<()>::new();
    let terrain = TerrainOptions { skirt_length: TerrainSkirtLength::None, drape_memory_limit: Some(1 << 20) };
    let options = MapOptions { size: Size::new(256, 256), terrain, ..Default::default() };
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    let mut map =
        Map::with_file_source(file_source, options, crate::tests::registry(), &crate::map::tests::pool()).expect("map");
    let reader = map.update_reader();
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    let update = reader.latest().expect("an update");
    assert_eq!(update.parameters.terrain, terrain);
}

fn hover(value: bool) -> FeatureState {
    [("hover".to_owned(), Value::Bool(value))].into_iter().collect()
}

/// The `hover` state of `points`' feature `1` that the latest update carries.
fn carried_hover(reader: &UpdateReader) -> Option<Value> {
    let update = reader.latest().expect("an update");
    let state = update.parameters.feature_states.get("points")?.get("", "1").get("hover")?.clone();
    Some(state)
}

#[test]
fn feature_state_changes_publish_updates_that_carry_the_state() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    let before = map.update_generation();

    assert!(map.set_feature_state("points", None, "1", hover(true)));
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(carried_hover(&reader), Some(Value::Bool(true)));
    assert_eq!(map.feature_state("points", Some(""), "1"), hover(true));

    // What changes nothing publishes nothing.
    assert!(!map.set_feature_state("points", None, "1", hover(true)));
    assert!(!map.set_feature_state("points", None, "1", FeatureState::new()));
    assert!(!map.remove_feature_state("points", None, Some("2"), None));
    assert!(!map.remove_feature_state("points", None, Some("1"), Some("count")));
    assert!(!map.remove_source_feature_state("other"));
    assert_eq!(map.update_generation(), before + 1);

    // A command group holds the update, and the state with it.
    map.begin_command_group();
    assert!(map.set_feature_state("points", None, "1", hover(false)));
    assert_eq!(carried_hover(&reader), Some(Value::Bool(true)));
    map.end_command_group().expect("the group ends");
    assert_eq!(carried_hover(&reader), Some(Value::Bool(false)));

    assert!(map.remove_feature_state("points", None, Some("1"), Some("hover")));
    assert!(reader.latest().expect("an update").parameters.feature_states.is_empty());

    map.set_feature_state("points", None, "1", hover(true));
    assert!(map.remove_source_feature_state("points"));
    assert_eq!(carried_hover(&reader), None);
}

#[test]
fn a_style_load_clears_the_feature_state_that_it_publishes() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    map.set_feature_state("points", None, "1", hover(true));
    map.set_feature_state("other", None, "1", hover(true));

    map.load_style_json(
        r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#,
    );
    map.process_events();
    assert!(map.feature_state("points", None, "1").is_empty());
    assert!(map.feature_state("other", None, "1").is_empty());
    let update = reader.latest().expect("an update");
    assert!(update.parameters.feature_states.is_empty());
    assert_eq!(1, update.parameters.style.get_sources().len());

    // State set once the style has loaded stays.
    map.set_feature_state("points", None, "1", hover(true));
    assert_eq!(carried_hover(&reader), Some(Value::Bool(true)));
}

/// A URL style parses in a later batch than the one that loads it, and its
/// load comes a batch later again: an update in between, as a renderer's
/// invalidation asks for, carries the new style's source.
#[test]
fn a_url_style_never_publishes_its_sources_with_the_old_feature_state() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Style, |_| {
        let mut response = Response::new();
        let style = r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#;
        response.data = Some(Arc::new(style.as_bytes().to_vec()));
        Some(response)
    });
    let options = MapOptions { size: Size::new(256, 256), ..Default::default() };
    let mut map =
        Map::with_file_source(file_source, options, crate::tests::registry(), &crate::map::tests::pool()).expect("map");
    map.jump_to(&CameraOptions::default());
    map.set_feature_state("points", None, "1", hover(true));

    // For each update that carries the style's source, whether it carried
    // any feature state.
    let reader = map.update_reader();
    // The waker's own reader, which goes before `reader`: dropping a reader
    // drops its waker under the slot's lock, which a reader's drop takes.
    let peek = Arc::new(Mutex::new(Some(reader.clone())));
    let carried: Arc<Mutex<Vec<bool>>> = Arc::default();
    let (waker_peek, waker_carried) = (Arc::clone(&peek), Arc::clone(&carried));
    reader.set_waker(Some(Arc::new(move || {
        if let Some(update) = waker_peek.lock().as_ref().and_then(UpdateReader::latest)
            && !update.parameters.style.get_sources().is_empty()
        {
            waker_carried.lock().push(!update.parameters.feature_states.is_empty());
        }
    })));

    map.load_style_url("http://style");
    map.process_events();
    run_loop.run_once(&mut ());
    map.renderer_events().push(RendererEvent::Invalidate);
    map.process_events();
    peek.lock().take();
    assert!(map.style_document_loaded());
    let carried = carried.lock().clone();
    assert!(!carried.is_empty(), "no update carried the style's source");
    assert!(!carried.contains(&true), "an update carried the old state: {carried:?}");
}

#[test]
fn the_update_reader_sees_the_map_go() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    let reader = map.update_reader();
    let generation = reader.generation();
    let (_reader, wakes) = count_wakes(&map);
    assert!(reader.latest().is_some());
    assert!(!reader.is_closed());

    drop(map);
    assert!(reader.is_closed());
    assert!(reader.latest().is_none());
    assert_eq!(reader.generation(), generation);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn an_update_reaches_a_reader_on_another_thread() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    let reader = map.update_reader();
    let (sender, receiver) = std::sync::mpsc::channel();
    reader.set_waker(Some(Arc::new(move || {
        let _ = sender.send(());
    })));
    let thread = std::thread::spawn(move || {
        receiver.recv().expect("a wake");
        reader.latest().map(|update| update.parameters.view.zoom())
    });
    map.jump_to(&CameraOptions { zoom: Some(4.0), ..Default::default() });
    let zoom = thread.join().expect("the reader thread").expect("an update");
    assert_near(zoom, 4.0);
}

#[test]
fn each_reader_wakes_until_it_drops() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    let (first, first_wakes) = count_wakes(&map);
    let (second, second_wakes) = count_wakes(&map);
    map.set_tile_settings(&TileSettings { prefetch_zoom_delta: 1, ..map.snapshot().tile });
    assert_eq!(first_wakes.load(Ordering::SeqCst), 1);
    assert_eq!(second_wakes.load(Ordering::SeqCst), 1);

    drop(first);
    // A clone starts without a waker.
    let _clone = second.clone();
    map.set_tile_settings(&TileSettings { prefetch_zoom_delta: 2, ..map.snapshot().tile });
    assert_eq!(first_wakes.load(Ordering::SeqCst), 1);
    assert_eq!(second_wakes.load(Ordering::SeqCst), 2);
}

/// Whether `update`'s only source updates synchronously.
fn publishes_synchronous(update: &Update) -> bool {
    let source = update.parameters.style.get_sources().iter().next().expect("a source");
    SourceData::<GeoJsonSourceType>::recover(source).is_some_and(|source| source.options.synchronous_update)
}

fn set_override_synchronous_update(map: &mut Map, value: bool) {
    let edit = GeoJsonSourceType::set_synchronous("points", value);
    map.edit_style(|style| style.apply(edit)).expect("a GeoJSON source");
}

#[test]
fn the_synchronous_update_override_publishes_an_update() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    map.load_style_json(
        r#"{"version":8,"sources":{"points":{"type":"geojson","synchronousUpdate":true,
            "data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#,
    );
    map.process_events();
    let before = map.update_generation();
    let published = reader.latest().expect("an update");
    assert!(publishes_synchronous(&published));

    set_override_synchronous_update(&mut map, false);
    assert_eq!(map.update_generation(), before + 1);
    assert!(!publishes_synchronous(&reader.latest().expect("an update")));
    // The update the renderer already has keeps its mode.
    assert!(publishes_synchronous(&published));

    // Setting the mode the source has publishes nothing.
    set_override_synchronous_update(&mut map, false);
    assert_eq!(map.update_generation(), before + 1);
}

const TERRAIN_STYLE: &str = r#"{"version":8,"sources":{
    "dem":{"type":"raster-dem","tiles":["https://example.com/{z}/{x}/{y}.png"]}},"layers":[]}"#;

fn published_terrain(reader: &UpdateReader) -> Option<(String, f32)> {
    let update = reader.latest().expect("an update");
    update.parameters.style.get_terrain().map(|terrain| (terrain.source.clone(), terrain.exaggeration))
}

#[test]
fn a_terrain_only_edit_publishes_a_style_with_the_terrain() {
    let run_loop = Queue::<()>::new();
    let mut map = map(&run_loop);
    map.load_style_json(TERRAIN_STYLE);
    map.process_events();
    map.drain_events();
    let reader = map.update_reader();
    let before = map.update_generation();

    map.edit_style(|style| style.set_root_property_json("terrain", r#"{"source":"dem"}"#)).unwrap();
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(published_terrain(&reader), Some(("dem".to_owned(), 1.0)));

    map.edit_style(|style| style.set_root_property_json("terrain", r#"{"source":"dem","exaggeration":2}"#)).unwrap();
    assert_eq!(map.update_generation(), before + 2);
    assert_eq!(published_terrain(&reader), Some(("dem".to_owned(), 2.0)));

    // What sets the terrain the style has publishes nothing.
    map.edit_style(|style| style.set_root_property_json("terrain", r#"{"source":"dem","exaggeration":2}"#)).unwrap();
    assert_eq!(map.update_generation(), before + 2);

    map.edit_style(|style| style.set_root_property_json("terrain", "null")).unwrap();
    assert_eq!(map.update_generation(), before + 3);
    assert_eq!(published_terrain(&reader), None);
}
