//! The requests of tiles and glyphs: how they are made, and how maps carry
//! on when they fail.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::map::MapEvent;
use mln_engine::storage::response::Reason;
use mln_engine::storage::{Kind, ResourceError, Response, StoragePolicy};
use mln_runtime::{
    DemandMode, FrameDisposition, RenderedQueryOptions, ResourceRequest, RuntimeEvent, RuntimeEventKind,
};
use parking_lot::Mutex;

use crate::support::still::{StillMap, file_response, not_found, serve};
use crate::support::{COMPLETION_TIMEOUT, wait};

/// A response that the server failed with `message`.
fn server_error(message: &str) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(Reason::Server, message)));
    response
}

fn no_content() -> Response {
    let mut response = Response::new();
    response.no_content = true;
    response
}

fn loading_finished(events: &[RuntimeEvent]) -> bool {
    events.iter().any(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::LoadingFinished)))
}

/// Draws frames of a continuous map whenever it changes, until `done` holds
/// for the events so far.
#[track_caller]
fn frames_until(map: &StillMap, done: impl Fn(&[RuntimeEvent]) -> bool) {
    let deadline = Instant::now() + COMPLETION_TIMEOUT;
    while !done(&map.events.events()) {
        assert!(Instant::now() < deadline, "the map didn't get there within {COMPLETION_TIMEOUT:?}");
        if map.frame_with(DemandMode::IfChanged).disposition != FrameDisposition::Rendered {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A volatile source's tile requests ask to stay out of the persistent
/// cache.
#[test]
fn a_volatile_source_requests_its_tiles_volatile() {
    let map = StillMap::continuous();
    let policies = Arc::new(Mutex::new(Vec::new()));
    {
        let policies = Arc::clone(&policies);
        serve(&map.runtime, move |resource| {
            if resource.kind == Kind::Tile {
                policies.lock().push(resource.storage_policy);
            }
            no_content()
        });
    }
    map.load_json(r#"{"version": 8, "sources": {}, "layers": []}"#);
    {
        // One group, so no frame takes the source before it is volatile.
        let group = map.map.command_group().unwrap();
        wait(map.map.add_style_source_json("vector", r#"{"type": "vector", "tiles": ["a/{z}/{x}/{y}"]}"#));
        wait(map.map.set_style_source_volatile("vector", true));
        let layer = r#"{"id": "water", "type": "fill", "source": "vector", "source-layer": "water"}"#;
        wait(map.map.add_style_layer_json(layer, None));
        wait(group.end());
    }
    map.jump(CameraOptions { zoom: Some(16.0), ..Default::default() });
    frames_until(&map, loading_finished);

    let policies = policies.lock();
    assert!(!policies.is_empty(), "no tile was requested");
    assert!(policies.iter().all(|policy| *policy == StoragePolicy::Volatile), "{policies:?}");
}

/// Serves requests of `kind` with an error while the returned flag is set,
/// then with `response`, and every other request with not found. Returns
/// the flag, and the count of the requests of `kind`.
fn fail_requests(map: &StillMap, kind: Kind, response: Response) -> (Arc<AtomicBool>, impl Fn() -> usize) {
    let failing = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&failing);
    let requests = serve(&map.runtime, move |resource| {
        if resource.kind != kind {
            not_found()
        } else if flag.load(Ordering::SeqCst) {
            server_error("failed")
        } else {
            response.clone()
        }
    });
    (failing, move || requests.count(kind))
}

/// The labels drawn at the center of a 256 by 256 map.
fn labels(map: &StillMap) -> usize {
    map.query_point(128.0, 128.0, RenderedQueryOptions::default()).len()
}

/// A still image fails with a resource that failed to load, and the next
/// one asks for it again, and draws what it then gets; the one after asks
/// for nothing. A tile laid out without a glyph range lays out again with
/// it.
#[test]
fn a_still_image_retries_what_failed_to_load() {
    struct Case {
        name: &'static str,
        kind: Kind,
        style: &'static str,
        response: fn() -> Response,
        /// Whether the map draws a label once the resource loads.
        labels: bool,
    }
    let cases = [
        Case {
            name: "vector tile",
            kind: Kind::Tile,
            style: r#"{"version": 8,
                "sources": {"tiles": {"type": "vector", "tiles": ["http://example.com/{z}/{x}/{y}.pbf"], "maxzoom": 0}},
                "layers": [{"id": "fill", "type": "fill", "source": "tiles", "source-layer": "any"}]}"#,
            response: no_content,
            labels: false,
        },
        Case {
            name: "raster tile",
            kind: Kind::Tile,
            style: r#"{"version": 8,
                "sources": {"tiles": {"type": "raster", "tiles": ["http://example.com/{z}/{x}/{y}.png"], "maxzoom": 0}},
                "layers": [{"id": "raster", "type": "raster", "source": "tiles"}]}"#,
            response: no_content,
            labels: false,
        },
        Case {
            name: "glyph range",
            kind: Kind::Glyphs,
            style: r#"{"version": 8,
                "glyphs": "http://example.com/{fontstack}/{range}.pbf",
                "sources": {"point": {"type": "geojson", "data": {"type": "Point", "coordinates": [0, 0]}}},
                "layers": [{"id": "label", "type": "symbol", "source": "point", "layout": {"text-field": "A"}}]}"#,
            response: || file_response("test/fixtures/resources/glyphs.pbf"),
            labels: true,
        },
    ];
    for case in &cases {
        let map = StillMap::still();
        let (failing, requests) = fail_requests(&map, case.kind, (case.response)());
        map.load_json(case.style);

        let error = map.try_render().expect_err(case.name);
        assert_eq!(error.message, "failed", "{}", case.name);
        let failed = requests();
        assert!(failed >= 1, "{}", case.name);
        if case.labels {
            assert_eq!(labels(&map), 0, "{}", case.name);
        }

        failing.store(false, Ordering::SeqCst);
        map.try_render().unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let answered = requests();
        assert_eq!(answered, failed + 1, "{}", case.name);
        if case.labels {
            assert_eq!(labels(&map), 1, "{}", case.name);
        }

        // Nothing failed, so nothing is asked for again.
        map.try_render().unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(requests(), answered, "{}", case.name);
    }
}

const FAILING_VECTOR_STYLE: &str = r#"{
  "version": 8,
  "sources": {
    "mapbox": {"type": "vector", "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]}
  },
  "layers": [{"id": "water", "type": "fill", "source": "mapbox", "source-layer": "water"}]
}"#;

const LINE_LAYER: &str = r#"{"id": "line", "type": "line", "source": "mapbox", "source-layer": "water"}"#;

/// A still image after one that a tile's error failed fails with the error
/// again, though a new layer lays the errored tile out again first.
#[test]
fn a_still_image_after_a_tile_error_reports_it_again() {
    let map = StillMap::still();
    serve(&map.runtime, |_| server_error("Tile failed"));
    map.load_json(FAILING_VECTOR_STYLE);
    assert_eq!(map.try_render().err().map(|error| error.message).as_deref(), Some("Tile failed"));

    wait(map.map.add_style_layer_json(LINE_LAYER, None));
    assert_eq!(map.try_render().err().map(|error| error.message).as_deref(), Some("Tile failed"));
}

/// A continuous map whose vector tile requests fail finishes loading, and
/// draws a full frame again after a new layer lays the failed tiles out
/// again.
#[test]
fn a_continuous_map_finishes_loading_when_its_tile_requests_fail() {
    let map = StillMap::continuous();
    serve(&map.runtime, |_| server_error("Tile failed"));
    map.load_json(FAILING_VECTOR_STYLE);
    frames_until(&map, loading_finished);

    wait(map.map.add_style_layer_json(LINE_LAYER, None));
    let deadline = Instant::now() + COMPLETION_TIMEOUT;
    while map.frame_with(DemandMode::WaitForUpdate).mode != RenderMode::Full {
        assert!(Instant::now() < deadline, "no full frame within {COMPLETION_TIMEOUT:?}");
    }
}

/// A continuous map whose tiles all fail to parse finishes loading, even
/// when they fail after the map stopped drawing frames.
#[test]
fn a_continuous_map_finishes_loading_when_its_tiles_fail() {
    let map = StillMap::continuous();
    let held: Arc<Mutex<Vec<ResourceRequest>>> = Arc::default();
    {
        let held = Arc::clone(&held);
        wait(map.runtime.set_resource_provider(move |request: ResourceRequest| {
            if request.resource().kind == Kind::Tile {
                held.lock().push(request);
            } else {
                let _ = request.respond(not_found());
            }
        }));
    }
    map.jump(CameraOptions { zoom: Some(11.0), ..Default::default() });
    map.load_json(
        r#"{
      "version": 8,
      "sources": {
        "invalidutf8": {"type": "vector", "tiles": ["http://example.com/{z}-{x}-{y}.vector.pbf"]}
      },
      "layers": [{
        "id": "mountain_peak", "type": "symbol", "source": "invalidutf8", "source-layer": "points",
        "layout": {"text-field": "{name}"}
      }]
    }"#,
    );

    // The tiles are answered only while the last frame needed no repaint,
    // so no frame is coming when they fail.
    let deadline = Instant::now() + COMPLETION_TIMEOUT;
    let mut repainting = true;
    while !loading_finished(&map.events.events()) {
        assert!(Instant::now() < deadline, "the map didn't finish loading within {COMPLETION_TIMEOUT:?}");
        if !repainting {
            for request in held.lock().drain(..) {
                let _ = request.respond(file_response("test/fixtures/map/invalid_utf8/invalidutf8feature.mvt"));
            }
        }
        let frame = map.frame_with(DemandMode::IfChanged);
        if frame.disposition == FrameDisposition::Rendered {
            repainting = frame.needs_repaint;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
