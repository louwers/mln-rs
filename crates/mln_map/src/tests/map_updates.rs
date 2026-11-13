//! The map's update slot: the updates it publishes for its renderer, their
//! readers, and command groups.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use feature::Value;
use mln_actor::RunLoop;
use mln_renderer::update_slot::Update;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::expression::expression::FeatureState;
use mln_style::sources::geojson_source::GeoJSONSource;
use mln_util::size::Size;

use crate::{CameraOptions, ErrorKind, Map, MapOptions, UpdateReader};

fn map() -> Map {
    let options = MapOptions { size: Size::new(256, 256), ..Default::default() };
    Map::with_file_source(Arc::new(StubFileSource::new(ResponseType::Asynchronous)), options).expect("map")
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
    let _loop = RunLoop::new();
    let mut map = map();
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
    map.set_prefetch_zoom_delta(1);
    assert_eq!(map.update_generation(), before);
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    map.end_command_group().expect("the outer group ends");
    assert_eq!(map.command_group_depth(), 0);
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let update = reader.latest().expect("an update");
    assert_eq!(update.generation, before + 1);
    assert_near(update.parameters.transform_state.get_zoom(), 3.0);
    assert_eq!(update.parameters.prefetch_zoom_delta, 1);

    let error = map.end_command_group().expect_err("no group is open");
    assert_eq!(error.kind, ErrorKind::InvalidState);
    assert_eq!(map.update_generation(), before + 1);
}

#[test]
fn a_hold_keeps_updates_back_apart_from_the_groups() {
    let _loop = RunLoop::new();
    let mut map = map();
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
    let _loop = RunLoop::new();
    let mut map = map();
    map.drain_events();
    let before = map.update_generation();
    map.begin_command_group();
    map.end_command_group().expect("the group ends");
    assert_eq!(map.update_generation(), before);
}

fn hover(value: bool) -> FeatureState {
    [("hover".to_owned(), Value::Bool(value))].into_iter().collect()
}

/// The `hover` state of `points`' feature `1` that the latest update carries.
fn carried_hover(reader: &UpdateReader) -> Option<Value> {
    let update = reader.latest().expect("an update");
    let state = update.parameters.feature_states.get("points")?.get("")?.get("1")?.get("hover")?.clone();
    Some(state)
}

#[test]
fn feature_state_changes_publish_updates_that_carry_the_state() {
    let _loop = RunLoop::new();
    let mut map = map();
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    let before = map.update_generation();

    assert!(map.set_feature_state("points", None, "1", hover(true)));
    assert_eq!(map.update_generation(), before + 1);
    assert_eq!(carried_hover(&reader), Some(Value::Bool(true)));
    assert_eq!(map.get_feature_state("points", Some(""), "1"), hover(true));

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
    let _loop = RunLoop::new();
    let mut map = map();
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    map.set_feature_state("points", None, "1", hover(true));
    map.set_feature_state("other", None, "1", hover(true));

    map.style_mut().load_json(
        r#"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}},"layers":[]}"#,
    );
    map.process_events();
    assert!(map.get_feature_state("points", None, "1").is_empty());
    assert!(map.get_feature_state("other", None, "1").is_empty());
    let update = reader.latest().expect("an update");
    assert!(update.parameters.feature_states.is_empty());
    assert_eq!(1, update.parameters.sources.len());

    // State set once the style has loaded stays.
    map.set_feature_state("points", None, "1", hover(true));
    assert_eq!(carried_hover(&reader), Some(Value::Bool(true)));
}

#[test]
fn the_update_reader_sees_the_map_go() {
    let _loop = RunLoop::new();
    let mut map = map();
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
    let _loop = RunLoop::new();
    let mut map = map();
    let reader = map.update_reader();
    let (sender, receiver) = std::sync::mpsc::channel();
    reader.set_waker(Some(Arc::new(move || {
        let _ = sender.send(());
    })));
    let thread = std::thread::spawn(move || {
        receiver.recv().expect("a wake");
        reader.latest().map(|update| update.parameters.transform_state.get_zoom())
    });
    map.jump_to(&CameraOptions { zoom: Some(4.0), ..Default::default() });
    let zoom = thread.join().expect("the reader thread").expect("an update");
    assert_near(zoom, 4.0);
}

#[test]
fn each_reader_wakes_until_it_drops() {
    let _loop = RunLoop::new();
    let mut map = map();
    let (first, first_wakes) = count_wakes(&map);
    let (second, second_wakes) = count_wakes(&map);
    map.set_prefetch_zoom_delta(1);
    assert_eq!(first_wakes.load(Ordering::SeqCst), 1);
    assert_eq!(second_wakes.load(Ordering::SeqCst), 1);

    drop(first);
    // A clone starts without a waker.
    let _clone = second.clone();
    map.set_prefetch_zoom_delta(2);
    assert_eq!(first_wakes.load(Ordering::SeqCst), 1);
    assert_eq!(second_wakes.load(Ordering::SeqCst), 2);
}

/// Whether `update`'s only source updates synchronously.
fn publishes_synchronous(update: &Update) -> bool {
    update.parameters.sources[0].is_update_synchronous()
}

fn set_override_synchronous_update(map: &mut Map, value: bool) {
    let source = map.style_mut().get_source("points").expect("the source");
    source.downcast_ref::<GeoJSONSource>().expect("a GeoJSON source").set_override_synchronous_update(value);
    map.process_events();
}

#[test]
fn the_synchronous_update_override_publishes_an_update() {
    let _loop = RunLoop::new();
    let mut map = map();
    map.jump_to(&CameraOptions::default());
    let reader = map.update_reader();
    map.style_mut().load_json(
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
