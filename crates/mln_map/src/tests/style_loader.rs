//! The style loader, through the map: style documents from JSON and URLs,
//! and sprites.

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_storage::response::{Reason, ResourceError};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_style::StyleLoadError;

use crate::{Map, MapEvent, MapOptions};

/// A path of this crate's test data.
fn testdata(relative: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative).to_string_lossy().into_owned()
}

fn map_over(file_source: StubFileSource, options: MapOptions, run_loop: &RunLoop) -> Map {
    Map::with_file_source(Arc::new(file_source), options, run_loop).expect("map")
}

/// A map whose style requests `respond` answers.
fn map_with_style_response(responder: impl Fn(&Resource) -> Option<Response> + 'static) -> (Map, RunLoop) {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Style, responder);
    (map_over(file_source, MapOptions::default(), &run_loop), run_loop)
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
    assert!(!map.style().is_loaded());
    assert_eq!(Some("http://some-url"), map.style_url());
    run_loop.run_once();
    assert!(map.has_reports());
    map.process_events();
    assert!(map.style().is_loaded());
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
    run_loop.run_once();
    let events = map.drain_events();
    assert_eq!(
        vec![StyleLoadError::Load("loading style failed: HTTP status code 404".to_owned())],
        load_failures(&events)
    );
    assert!(!map.style().is_loaded());
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
        run_loop.run_once();
        let events = map.drain_events();
        assert_eq!(vec![StyleLoadError::Load(format!("loading style failed: {message}"))], load_failures(&events));
        assert!(!map.style().is_loaded());
    }
}

#[test]
fn load_url_ignores_no_content_and_not_modified_after_a_style() {
    let (mut map, run_loop) = map_with_style_response(|_| Some(style_response(REMOTE)));
    map.load_style_url("http://some-url");
    run_loop.run_once();
    map.drain_events();
    assert!(map.style().is_loaded());
    // A revalidation that confirms the style the first response brought.
    for (no_content, not_modified) in [(true, false), (false, true)] {
        let mut response = Response::new();
        response.no_content = no_content;
        response.not_modified = not_modified;
        map.style_loader().push_style_response(response);
        let events = map.drain_events();
        assert!(load_failures(&events).is_empty(), "{events:?}");
    }
    assert!(map.style().is_loaded());
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
    assert!(!map.style().is_loaded());
}

// gtest: Style.LoadJSONCancelsPendingLoadURL
#[test]
fn load_json_cancels_pending_load_url() {
    let run_loop = RunLoop::new();
    let file_source = Arc::new(StubFileSource::new(&run_loop, ResponseType::Manual));
    file_source.set_response(Kind::Style, |_| Some(style_response(r#"{"version": 8, "name": "Streets"}"#)));
    let mut map = Map::with_file_source(Arc::clone(&file_source) as _, MapOptions::default(), &run_loop).expect("map");

    // Start loading a URL (this will be pending)
    map.load_style_url("http://some-url");

    // Before the URL request completes, load JSON directly
    map.load_style_json(r#"{"version": 8, "name": "Test Style", "sources": {}, "layers": []}"#);

    // The style should now be loaded with our JSON content
    assert_eq!("Test Style", map.style().get_name());
    assert_eq!(None, map.style_url());
    assert!(map.style().get_json().contains("Test Style"));

    file_source.respond_to_all();
    run_loop.run_once();
    map.process_events();

    // The style should still show our JSON content, not the URL content
    assert_eq!("Test Style", map.style().get_name());
}

// gtest: Style.PublicLoadStateDoesNotWaitForResources
#[test]
fn public_load_state_does_not_wait_for_resources() {
    let run_loop = RunLoop::new();
    let mut map = map_over(StubFileSource::new(&run_loop, ResponseType::Manual), MapOptions::default(), &run_loop);
    assert!(!map.style().is_loaded());

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
    assert!(map.style().is_loaded());

    map.load_style_url("http://some-url");
    assert!(!map.style().is_loaded());
}

// Sprites. Ported gtests: test/sprite/sprite_loader.test.cpp.

fn default_sprite_path() -> String {
    testdata("test/fixtures/resources/sprite")
}

fn sprite_style(sprite_path: &str) -> String {
    format!(r#"{{"version": 8, "sprite": "{sprite_path}", "sources": {{}}, "layers": []}}"#)
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
    file_source: StubFileSource,
    sprite_path: &str,
    pixel_ratio: f32,
    run_loop: &RunLoop,
) -> (Map, MapEvent) {
    let options = MapOptions { pixel_ratio, ..Default::default() };
    let mut map = map_over(file_source, options, run_loop);
    let handle = run_loop.handle();
    map.set_reports_waker(Some(Arc::new(move || handle.stop())));
    map.load_style_json(&sprite_style(sprite_path));
    loop {
        for event in map.drain_events() {
            if matches!(event, MapEvent::SpriteLoaded(_) | MapEvent::SpriteError(..)) {
                return (map, event);
            }
        }
        run_loop.run();
    }
}

fn loading_success(sprite_path: String, expected_images: usize, pixel_ratio: f32) {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::SpriteImage, successful_response(".png", sprite_path.clone(), pixel_ratio));
    file_source.set_response(Kind::SpriteJSON, successful_response(".json", sprite_path.clone(), pixel_ratio));

    let (map, event) = load_sprite(file_source, &sprite_path, pixel_ratio, &run_loop);
    assert!(matches!(event, MapEvent::SpriteLoaded(Some(_))), "{event:?}");
    assert_eq!(expected_images, map.style().impl_.get_image_impls().len());
    assert!(map.style_loaded());
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_sprite_1x() {
    loading_success(default_sprite_path(), 418, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_1x() {
    loading_success(testdata("test/fixtures/resources/versatiles-sprite/sprite"), 112, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_2x() {
    loading_success(testdata("test/fixtures/resources/versatiles-sprite/sprite"), 112, 2.0);
}

/// The message of the sprite error that loading the default sprite with
/// these responses ends in.
fn sprite_error(
    image: impl Fn(&Resource) -> Option<Response> + 'static,
    json: impl Fn(&Resource) -> Option<Response> + 'static,
) -> String {
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(&run_loop, ResponseType::Asynchronous);
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
    let run_loop = RunLoop::new();
    let file_source = StubFileSource::new(&run_loop, ResponseType::Asynchronous);
    let stop = run_loop.handle();
    let end = move |_: &Resource| -> Option<Response> {
        stop.stop();
        None
    };
    file_source.set_response(Kind::SpriteImage, end.clone());
    file_source.set_response(Kind::SpriteJSON, end);

    let mut map = map_over(file_source, MapOptions::default(), &run_loop);
    map.load_style_json(&sprite_style(&default_sprite_path()));
    run_loop.run();
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
