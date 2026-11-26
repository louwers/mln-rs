//! The style loader, through the map: style documents from JSON and URLs,
//! and sprites.

use std::sync::Arc;

use mln_style::{Edit, StyleLoadError};
use mln_util::queue::Queue;

use crate::map::{Map, MapEvent, MapOptions};
use crate::storage::response::{Reason, ResourceError};
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::storage::{Kind, Resource, Response};

/// A path of this crate's test data.
fn testdata(relative: &str) -> String {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata")
        .join(relative)
        .to_string_lossy()
        .into_owned()
}

fn map_over(file_source: Arc<StubFileSource>, options: MapOptions) -> Map {
    Map::with_file_source(file_source, options, &crate::map::tests::pool()).expect("map")
}

/// A map whose style requests `respond` answers.
fn map_with_style_response(
    responder: impl Fn(&Resource) -> Option<Response> + Send + Sync + 'static,
) -> (Map, Queue<()>) {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Style, responder);
    (map_over(file_source, MapOptions::default()), run_loop)
}

fn style_response(json: &str) -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(json.as_bytes().to_vec()));
    response
}

fn load_failures(events: &[MapEvent]) -> Vec<StyleLoadError> {
    events
        .iter()
        .filter_map(|event| match event {
            MapEvent::LoadingFailed(error) => Some(error.clone()),
            _ => None,
        })
        .collect()
}

const REMOTE: &str = r#"{"version": 8, "name": "Remote", "sources": {}, "layers": []}"#;

#[test]
fn load_url_parses_the_response() {
    let (mut map, run_loop) = map_with_style_response(|resource| {
        assert_eq!(resource.url, "http://some-url");
        Some(style_response(REMOTE))
    });
    map.load_style_url("http://some-url");
    assert!(!map.style_document_loaded());
    assert_eq!(Some("http://some-url"), map.style_url());
    run_loop.run_once(&mut ());
    assert!(map.has_reports());
    map.process_events();
    assert!(map.style_document_loaded());
    assert_eq!("Remote", map.style().get_name());
    assert_eq!(Some("http://some-url"), map.style_url());
}

#[test]
fn load_url_error() {
    let (mut map, run_loop) = map_with_style_response(|_| {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::NotFound, "HTTP status code 404")));
        Some(response)
    });
    map.load_style_url("http://some-url");
    run_loop.run_once(&mut ());
    let events = map.drain_events();
    assert_eq!(
        vec![StyleLoadError::Load("loading style failed: HTTP status code 404".to_owned())],
        load_failures(&events)
    );
    assert!(!map.style_document_loaded());
}

#[test]
fn load_url_answered_without_a_style_fails() {
    for (no_content, message) in [
        (true, "the style request returned no content"),
        (false, "the style request returned not modified, but no earlier response delivered the style"),
    ] {
        let (mut map, run_loop) = map_with_style_response(move |_| {
            let mut response = Response::new();
            response.no_content = no_content;
            response.not_modified = !no_content;
            Some(response)
        });
        map.load_style_url("http://some-url");
        run_loop.run_once(&mut ());
        let events = map.drain_events();
        assert_eq!(vec![StyleLoadError::Load(format!("loading style failed: {message}"))], load_failures(&events));
        assert!(!map.style_document_loaded());
    }
}

#[test]
fn load_url_ignores_no_content_and_not_modified_after_a_style() {
    let (mut map, run_loop) = map_with_style_response(|_| Some(style_response(REMOTE)));
    map.load_style_url("http://some-url");
    run_loop.run_once(&mut ());
    map.drain_events();
    assert!(map.style_document_loaded());
    // A revalidation that confirms the style the first response brought.
    for (no_content, not_modified) in [(true, false), (false, true)] {
        let mut response = Response::new();
        response.no_content = no_content;
        response.not_modified = not_modified;
        map.style_loader().push_style_response(response);
        let events = map.drain_events();
        assert!(load_failures(&events).is_empty(), "{events:?}");
    }
    assert!(map.style_document_loaded());
    assert_eq!("Remote", map.style().get_name());
}

fn connection_error() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Connection, "offline")));
    response
}

/// The file source answers the style's request again as it retries an error
/// and revalidates: a streak of errors fails the load once.
#[test]
fn a_streak_of_errors_fails_the_load_once() {
    let (mut map, run_loop) = map_with_style_response(|_| Some(connection_error()));
    map.load_style_url("http://some-url");
    run_loop.run_once(&mut ());
    let mut events = map.drain_events();
    for _ in 0..2 {
        map.style_loader().push_style_response(connection_error());
        events.extend(map.drain_events());
    }
    assert_eq!(1, load_failures(&events).len());
    // A success ends the streak.
    map.style_loader().push_style_response(style_response(REMOTE));
    map.drain_events();
    assert!(map.style_document_loaded());
}

/// Revalidations that fail keep the style, and don't fail its load.
#[test]
fn failed_revalidations_keep_the_style() {
    let (mut map, run_loop) = map_with_style_response(|_| Some(style_response(REMOTE)));
    map.load_style_url("http://some-url");
    run_loop.run_once(&mut ());
    map.drain_events();
    for _ in 0..3 {
        map.style_loader().push_style_response(connection_error());
        let events = map.drain_events();
        assert!(load_failures(&events).is_empty(), "{events:?}");
    }
    assert!(map.style_document_loaded());
    assert_eq!("Remote", map.style().get_name());
}

#[test]
fn load_json_parse_error() {
    let (mut map, _run_loop) = map_with_style_response(|_| None);
    map.load_style_json("[]");
    let events = map.drain_events();
    assert_eq!(
        vec![StyleLoadError::Parse("Failed to parse style: style must be an object".to_owned())],
        load_failures(&events)
    );
    assert!(!map.style_document_loaded());
}

// gtest: Style.LoadJSONCancelsPendingLoadURL
#[test]
fn load_json_cancels_pending_load_url() {
    let run_loop = Queue::<()>::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    file_source.set_response(Kind::Style, |_| Some(style_response(r#"{"version": 8, "name": "Streets"}"#)));
    let mut map = Map::with_file_source(
        Arc::<StubFileSource>::clone(&file_source),
        MapOptions::default(),
        &crate::map::tests::pool(),
    )
    .expect("map");

    // Start loading a URL (this will be pending)
    map.load_style_url("http://some-url");

    // Before the URL request completes, load JSON directly
    map.load_style_json(r#"{"version": 8, "name": "Test Style", "sources": {}, "layers": []}"#);

    // The style should now be loaded with our JSON content
    assert_eq!("Test Style", map.style().get_name());
    assert_eq!(None, map.style_url());
    assert!(map.style().get_json().contains("Test Style"));

    file_source.respond_to_all();
    run_loop.run_once(&mut ());
    map.process_events();

    // The style should still show our JSON content, not the URL content
    assert_eq!("Test Style", map.style().get_name());
}

// gtest: Style.PublicLoadStateDoesNotWaitForResources
#[test]
fn public_load_state_does_not_wait_for_resources() {
    let mut map = map_over(Arc::new(StubFileSource::new(ResponseType::Manual)), MapOptions::default());
    assert!(!map.style_document_loaded());

    map.load_style_json(
        r#"{
        "version": 8,
        "sources": {
            "pending": {
                "type": "geojson",
                "data": { "type": "FeatureCollection", "features": [] }
            }
        },
        "layers": []
    }"#,
    );
    assert!(map.style_document_loaded());

    map.load_style_url("http://some-url");
    assert!(!map.style_document_loaded());
}

// Sprites. Ported gtests: test/sprite/sprite_loader.test.cpp.

fn default_sprite_path() -> String {
    testdata("test/fixtures/resources/sprite")
}

fn sprite_style(sprite_path: &str) -> String {
    serde_json::json!({"version": 8, "sprite": sprite_path, "sources": {}, "layers": []}).to_string()
}

fn successful_response(
    extension: &'static str,
    sprite_path: String,
    pixel_ratio: f32,
) -> impl Fn(&Resource) -> Option<Response> {
    move |resource: &Resource| {
        assert_eq!(format!("{sprite_path}{}{extension}", if pixel_ratio == 2.0 { "@2x" } else { "" }), resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(std::fs::read(&resource.url).unwrap()));
        Some(response)
    }
}

fn failed_sprite_response() -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Other, "Failed by the test case")));
    response
}

fn corrupt_sprite_response() -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(b"CORRUPT".to_vec()));
    response
}

/// Loads a style whose sprite is at `sprite_path`, and runs the loop until
/// the sprite loads or fails. Returns the map and that event.
fn load_sprite(
    file_source: Arc<StubFileSource>,
    sprite_path: &str,
    pixel_ratio: f32,
    run_loop: &Queue<()>,
) -> (Map, MapEvent) {
    let options = MapOptions { pixel_ratio, ..Default::default() };
    let mut map = map_over(file_source, options);
    let handle = run_loop.clone();
    map.set_reports_waker(Some(Arc::new(move || handle.stop())));
    map.load_style_json(&sprite_style(sprite_path));
    loop {
        for event in map.drain_events() {
            if matches!(event, MapEvent::SpriteLoaded(_) | MapEvent::SpriteError(..)) {
                return (map, event);
            }
        }
        run_loop.run(&mut ());
    }
}

fn loading_success(sprite_path: &str, expected_images: usize, pixel_ratio: f32) {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::SpriteImage, successful_response(".png", sprite_path.to_owned(), pixel_ratio));
    file_source.set_response(Kind::SpriteJSON, successful_response(".json", sprite_path.to_owned(), pixel_ratio));

    let (map, event) = load_sprite(file_source, sprite_path, pixel_ratio, &run_loop);
    assert!(matches!(event, MapEvent::SpriteLoaded(Some(_))), "{event:?}");
    assert_eq!(expected_images, map.style().get_images().len());
    assert!(map.style_loaded());
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_sprite_1x() {
    loading_success(&default_sprite_path(), 418, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_1x() {
    loading_success(&testdata("test/fixtures/resources/versatiles-sprite/sprite"), 112, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_2x() {
    loading_success(&testdata("test/fixtures/resources/versatiles-sprite/sprite"), 112, 2.0);
}

/// The message of the sprite error that loading the default sprite with
/// these responses ends in.
fn sprite_error(
    image: impl Fn(&Resource) -> Option<Response> + Send + Sync + 'static,
    json: impl Fn(&Resource) -> Option<Response> + Send + Sync + 'static,
) -> String {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::SpriteImage, image);
    file_source.set_response(Kind::SpriteJSON, json);
    let (map, event) = load_sprite(file_source, &default_sprite_path(), 1.0, &run_loop);
    let MapEvent::SpriteError(Some(_), error) = event else { panic!("{event:?}") };
    // A failed sprite stops failing still images only with the next load.
    assert_eq!(Some(&error), map.last_style_error());
    error.to_string()
}

// gtest: SpriteLoader.JSONLoadingFail
#[test]
fn json_loading_fail() {
    let error =
        sprite_error(successful_response(".png", default_sprite_path(), 1.0), |_| Some(failed_sprite_response()));
    assert_eq!("Failed by the test case", error);
}

// gtest: SpriteLoader.ImageLoadingFail
#[test]
fn image_loading_fail() {
    let error =
        sprite_error(|_| Some(failed_sprite_response()), successful_response(".json", default_sprite_path(), 1.0));
    assert_eq!("Failed by the test case", error);
}

// gtest: SpriteLoader.JSONLoadingCorrupted
#[test]
fn json_loading_corrupted() {
    let error =
        sprite_error(successful_response(".png", default_sprite_path(), 1.0), |_| Some(corrupt_sprite_response()));
    // serde_json's message, after the parser's prefix.
    assert!(error.starts_with("Failed to parse JSON: "), "{error}");
}

// gtest: SpriteLoader.ImageLoadingCorrupted
#[test]
fn image_loading_corrupted() {
    // Not asserting on platform-specific error text.
    sprite_error(|_| Some(corrupt_sprite_response()), successful_response(".json", default_sprite_path(), 1.0));
}

// gtest: SpriteLoader.LoadingCancel
#[test]
fn loading_cancel() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    let stop = run_loop.clone();
    let end = move |_: &Resource| -> Option<Response> {
        stop.stop();
        None
    };
    file_source.set_response(Kind::SpriteImage, end.clone());
    file_source.set_response(Kind::SpriteJSON, end);

    let mut map = map_over(file_source, MapOptions::default());
    map.load_style_json(&sprite_style(&default_sprite_path()));
    run_loop.run(&mut ());
    let events = map.drain_events();
    assert!(events.iter().any(|event| matches!(event, MapEvent::SpriteRequested(Some(_)))), "{events:?}");
    assert!(!events.iter().any(|event| matches!(event, MapEvent::SpriteLoaded(_))), "Should never be called");
    assert!(!map.style_loaded());
}

#[test]
fn a_style_without_a_sprite_loads_an_empty_one() {
    let (mut map, _run_loop) = map_with_style_response(|_| None);
    map.load_style_json(REMOTE);
    let events = map.drain_events();
    assert!(events.iter().any(|event| matches!(event, MapEvent::SpriteLoaded(None))), "{events:?}");
    assert!(map.style_loaded());
}

// Edits of the loaded document.

const DOTS: &str = r#"{"id":"dots","type":"circle","source":"points"}"#;

#[test]
fn edits_fail_while_the_style_is_not_loaded() {
    let (mut map, _run_loop) = map_with_style_response(|_| None);
    assert!(!map.style_document_loaded());
    let not_loaded = |result: Result<(), mln_style::error::Error>| {
        let error = result.expect_err("the edit should fail");
        assert_eq!(
            (mln_style::error::ErrorKind::InvalidState, "style is not loaded"),
            (error.kind, error.message.as_str())
        );
    };
    not_loaded(map.edit_loaded_style(|style| style.add_layer_json(DOTS, None)));
    not_loaded(map.edit_loaded_style(|style| style.set_layer_property_json("dots", "circle-radius", "4")));
    not_loaded(map.edit_loaded_style(|style| style.set_root_property_json("light", r#"{"intensity":0.5}"#)));
    not_loaded(map.edit_loaded_style(|style| style.set_light_property_json("intensity", "0.5")));
    not_loaded(map.edit_loaded_style(|style| style.set_global_state_property_json("theme", "true")));
    assert!(map.style().get_sources().is_empty() && map.style().get_layers().len() == 0);
}

// gtest: Style.GlobalStatePropertyRequiresLoadedStyle
#[test]
fn global_state_property_requires_loaded_style() {
    let (mut map, _run_loop) = map_with_style_response(|_| None);
    let set = |map: &mut Map, value: bool| {
        let value = feature::Value::Bool(value);
        map.edit_loaded_style(|style| style.apply(Edit::SetGlobalState { name: "showLabels".to_owned(), value }))
    };
    assert!(set(&mut map, false).is_err());
    assert!(map.style().get_global_state().is_empty());

    map.load_style_json(r#"{"version": 8, "state": {"showLabels": {"default": true}}, "sources": {}, "layers": []}"#);
    assert!(map.style_document_loaded());
    assert_eq!(feature::Value::Bool(true), map.style().get_global_state()["showLabels"]);
    set(&mut map, false).unwrap();
    assert_eq!(feature::Value::Bool(false), map.style().get_global_state()["showLabels"]);
}

#[test]
fn edits_fail_during_a_url_load_and_the_old_version_stays_published() {
    let run_loop = Queue::<()>::new();
    let file_source = Arc::new(StubFileSource::new(ResponseType::Manual));
    file_source.set_response(Kind::Style, |_| Some(style_response(REMOTE)));
    let options = MapOptions { map_mode: crate::map::MapMode::Continuous, ..MapOptions::default() };
    let mut map =
        Map::with_file_source(Arc::<StubFileSource>::clone(&file_source), options, &crate::map::tests::pool())
            .expect("map");
    let reader = map.update_reader();
    map.load_style_json(r#"{"version": 8, "name": "Local", "sources": {}, "layers": []}"#);
    let published = Arc::clone(&reader.latest().expect("an update").parameters.style);
    let local = Arc::clone(map.style());

    map.load_style_url("http://some-url");
    assert!(!map.style_document_loaded());
    let error = map.edit_loaded_style(|style| style.add_layer_json(r#"{"id":"paper","type":"background"}"#, None));
    assert_eq!(mln_style::error::ErrorKind::InvalidState, error.unwrap_err().kind);
    // The version from before the load stays, and stays published.
    assert!(Arc::ptr_eq(&local, map.style()));
    assert!(Arc::ptr_eq(&local, &published));
    assert!(Arc::ptr_eq(&published, &reader.latest().expect("an update").parameters.style));

    file_source.respond_to_all();
    run_loop.run_once(&mut ());
    map.process_events();
    assert!(map.style_document_loaded());
    assert_eq!("Remote", map.style().get_name());
    map.edit_loaded_style(|style| style.add_layer_json(r#"{"id":"paper","type":"background"}"#, None)).unwrap();
}
