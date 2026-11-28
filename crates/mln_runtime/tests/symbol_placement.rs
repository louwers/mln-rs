//! Symbol placement in a continuous map: it runs again when something it
//! places from changes, and only then, and the symbols it hides fade out,
//! draw nothing once hidden, and draw again once it shows them.

use std::time::{Duration, Instant};

use geojson::GeoJson;
use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapEvent, MapMode, RenderFrameStatus};
use mln_runtime::{DemandMode, FeatureStateSelector, FrameDisposition, FrameResult, Map, RuntimeEventKind};
use mln_style::bitmap::PremultipliedImage;
use mln_style::feature::{FeatureState, Identifier, Value};
use mln_style::geo::LatLng;
use mln_style::size::Size;
use serde_json::json;

use crate::support::still::{StillMap, StillOptions};
use crate::support::{TIMEOUT, wait};

// MARK: - Frames

/// Draws a frame of `map` in `mode`. Returns the frame's result, and for a
/// frame that rendered, the status that the map reported for it.
fn frame(map: &StillMap, mode: DemandMode) -> (FrameResult, Option<RenderFrameStatus>) {
    let result = map.frame_with(mode);
    if result.disposition != FrameDisposition::Rendered {
        return (result, None);
    }
    // The renderer counts the session's frames as the session does.
    let event = map.events.wait_for(|event| {
        matches!(&event.kind, RuntimeEventKind::Map(MapEvent::RenderFrameFinished(status))
            if u64::try_from(status.rendering_stats.num_frames) == Ok(result.frame_generation))
    });
    let Some(RuntimeEventKind::Map(MapEvent::RenderFrameFinished(status))) = event.map(|event| event.kind) else {
        panic!("no status for frame {}", result.frame_generation);
    };
    (result, Some(status))
}

/// Whether the map reported, after its event `since`, that it went idle.
fn idle_since(map: &StillMap, since: usize) -> bool {
    map.events.events()[since..].iter().any(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::Idle)))
}

/// Draws each frame that `map` asks for, as a display does, until `done`
/// accepts the status of one or the map goes idle. Returns the statuses of
/// the frames that rendered.
///
/// A frame can show everything loaded with nothing left to animate while
/// the map still works on a change in the background, such as new GeoJSON
/// data: only the map's `Idle` says that no frame follows.
#[track_caller]
fn frames_until(map: &StillMap, mut done: impl FnMut(&RenderFrameStatus) -> bool) -> Vec<RenderFrameStatus> {
    let since = map.events.events().len();
    let deadline = Instant::now() + TIMEOUT;
    let mut statuses = Vec::new();
    loop {
        match frame(map, DemandMode::IfChanged).1 {
            Some(status) => {
                let finished = done(&status);
                statuses.push(status);
                if finished {
                    return statuses;
                }
            }
            None if idle_since(map, since) => return statuses,
            None => {
                assert!(Instant::now() < deadline, "the map didn't go idle in {TIMEOUT:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// Draws the frames that `map` asks for until it goes idle. Returns the
/// statuses of the frames that rendered.
#[track_caller]
fn settle(map: &StillMap) -> Vec<RenderFrameStatus> {
    frames_until(map, |_| false)
}

/// A white icon 16 pixels a side.
fn marker() -> mln_style::image::Impl {
    let image = PremultipliedImage::from_vec(Size::new(16, 16), vec![255; 16 * 16 * 4]);
    mln_style::image::Impl::new("marker".to_owned(), image, 1.0, false, Vec::new(), Vec::new(), None, None, None)
        .expect("a style image")
}

fn lat_lng(latitude: f64, longitude: f64) -> LatLng {
    LatLng::new(latitude, longitude).expect("a coordinate")
}

// MARK: - Collisions

/// A continuous 256 by 256 map of `count` icons `spacing` degrees of
/// longitude apart from (5, 5), each with its own sort key, centered on
/// (5, 5) at zoom 4.
fn colliding_icons(count: usize, spacing: f64) -> StillMap {
    let map = StillMap::new(StillOptions { mode: MapMode::Continuous, ..StillOptions::default() });
    let features: Vec<_> = (0..count)
        .map(|i| {
            json!({
                "type": "Feature",
                "properties": {"rank": i},
                "geometry": {"type": "Point", "coordinates": [5.0 + i as f64 * spacing, 5.0]},
            })
        })
        .collect();
    let style = json!({
        "version": 8,
        "sources": {
            "points": {"type": "geojson", "maxzoom": 0, "data": {"type": "FeatureCollection", "features": features}},
        },
        "layers": [{
            "id": "symbols",
            "type": "symbol",
            "source": "points",
            "layout": {"symbol-sort-key": ["get", "rank"], "icon-image": "marker"},
        }],
    });
    map.load_json(&style.to_string());
    wait(map.map.set_style_image(marker()));
    map.jump(CameraOptions { center: Some(lat_lng(5.0, 5.0)), zoom: Some(4.0), ..CameraOptions::default() });
    map
}

/// The number of draw calls of the frame of `status`.
fn draws(status: &RenderFrameStatus) -> i32 {
    status.rendering_stats.num_draw_calls
}

// gtest: SymbolRendering.ContinuousCollisionVisibilityFadesAndReappears
#[test]
fn continuous_collision_visibility_fades_and_reappears() {
    let map = colliding_icons(2, 0.5);
    map.jump(CameraOptions { center: Some(lat_lng(5.0, 5.25)), zoom: Some(4.9), ..CameraOptions::default() });

    let initial = settle(&map);
    assert_eq!(draws(initial.last().expect("a frame")), 2, "initial");
    let initial_image = map.image();

    // A committed collision keeps the fading symbol's drawable enabled,
    // until the symbol has faded out.
    map.jump(CameraOptions { zoom: Some(4.1), ..CameraOptions::default() });
    let colliding = frames_until(&map, |status| status.mode == RenderMode::Full && draws(status) == 1);
    let hidden = colliding.last().filter(|status| status.mode == RenderMode::Full && draws(status) == 1);
    assert!(hidden.is_some(), "the map went idle with both symbols drawn");
    let fade_out = colliding.iter().any(|status| {
        status.mode == RenderMode::Full && status.placement_changed && status.needs_repaint && draws(status) == 2
    });
    assert!(fade_out, "the colliding symbol didn't fade out");

    map.jump(CameraOptions { zoom: Some(4.9), ..CameraOptions::default() });
    let reappeared = settle(&map);
    assert_eq!(draws(reappeared.last().expect("a frame")), 2, "reappeared");
    assert!(map.image() == initial_image, "the reappeared frame differs from the initial one");
}

// gtest: SymbolRendering.FullyHiddenLayerCanReappear
//
// C++ draws still images, whose draw calls the runtime doesn't report: here
// a continuous map settles after each change instead.
#[test]
fn fully_hidden_layer_can_reappear() {
    let map = colliding_icons(256, 0.0);
    settle(&map);
    let initial = map.image();

    let blocker = json!({"id": "blocker", "type": "symbol", "source": "points", "layout": {"icon-image": "marker"}});
    wait(map.map.add_style_layer_json(blocker.to_string(), None));
    let hidden = settle(&map);
    assert_eq!(draws(hidden.last().expect("a frame")), 1);
    assert!(map.image() == initial, "the blocker draws otherwise than the layer it hides");

    wait(map.map.set_style_layer_property("blocker", "visibility", r#""none""#));
    let visible = settle(&map);
    assert_eq!(draws(visible.last().expect("a frame")), 1);
    assert!(map.image() == initial, "the layer draws otherwise once it shows again");
}

// MARK: - What placement reads

/// A `FeatureCollection` of points at `coordinates` (longitude, latitude),
/// with their indexes as their ids.
fn points(coordinates: &[(f64, f64)]) -> GeoJson {
    let features: Vec<_> = coordinates
        .iter()
        .enumerate()
        .map(|(id, (longitude, latitude))| {
            json!({
                "type": "Feature",
                "id": id,
                "properties": {},
                "geometry": {"type": "Point", "coordinates": [longitude, latitude]},
            })
        })
        .collect();
    serde_json::from_value(json!({"type": "FeatureCollection", "features": features})).expect("GeoJSON")
}

/// A continuous map of icons, from two sources, over circles from a third.
/// Paint properties don't transition, so only placement and its fades keep
/// the map drawing.
fn labels_and_vehicles() -> StillMap {
    let map = StillMap::continuous();
    let source = |coordinates: &[(f64, f64)]| json!({"type": "geojson", "data": serde_json::to_value(points(coordinates)).expect("JSON")});
    let style = json!({
        "version": 8,
        "transition": {"duration": 0},
        "sources": {
            "labels": source(&[(4.0, 5.0), (6.0, 5.0)]),
            "more-labels": source(&[(5.0, 4.0)]),
            "vehicles": source(&[(5.0, 6.0), (4.0, 4.0)]),
        },
        "layers": [
            {"id": "vehicles", "type": "circle", "source": "vehicles",
             "paint": {"circle-color": ["case", ["boolean", ["feature-state", "hover"], false], "red", "blue"]}},
            {"id": "labels", "type": "symbol", "source": "labels", "layout": {"icon-image": "marker"}},
            {"id": "more-labels", "type": "symbol", "source": "more-labels", "layout": {"icon-image": "marker"}},
        ],
    });
    map.load_json(&style.to_string());
    wait(map.map.set_style_image(marker()));
    map.jump(CameraOptions { center: Some(lat_lng(5.0, 5.0)), zoom: Some(4.0), ..CameraOptions::default() });
    map
}

/// Settles the map with every symbol at the end of its fade: until a frame
/// that it draws unasked places nothing and asks for no frames. A change
/// made next finds no placement to finish.
///
/// A map can go idle with a placement that stopped short of the end of its
/// fade: with the style's transitions off, a placement that shows symbols
/// whose tiles came after the last one leaves them unsettled, and asks for
/// no placement after it.
fn settle_placed(map: &StillMap) {
    for _ in 0..100 {
        settle(map);
        let (_, status) = frame(map, DemandMode::Force);
        let status = status.expect("a forced frame renders");
        if !status.placement_changed && !status.needs_repaint {
            return;
        }
    }
    panic!("the map's placement didn't settle");
}

/// A change to a map, by name.
type Change = (&'static str, fn(&Map));

fn set_points(map: &Map, source_id: &str, coordinates: &[(f64, f64)]) {
    wait(map.set_geojson_source_data(source_id, serde_json::to_string(&points(coordinates)).expect("JSON")));
}

/// Changes that no symbol's placement reads.
fn changes_that_move_no_label() -> Vec<Change> {
    vec![
        ("new data for the circles", |map| set_points(map, "vehicles", &[(5.5, 6.0), (4.5, 4.0)])),
        ("a circle paint property", |map| {
            wait(map.set_style_layer_property("vehicles", "circle-radius", "8"));
        }),
        ("a symbol paint property placement doesn't read", |map| {
            wait(map.set_style_layer_property("labels", "icon-opacity", "0.5"));
        }),
        ("a circle's feature state", |map| {
            let mut state = FeatureState::new();
            state.insert("hover".into(), Value::Bool(true));
            wait(map.set_feature_state(&FeatureStateSelector::feature("vehicles", Identifier::Uint(0)), state));
        }),
    ]
}

/// Changes to what placement places from.
fn changes_to_placement_inputs() -> Vec<Change> {
    vec![
        ("the camera", |map| {
            let camera = CameraOptions { bearing: Some(30.0), ..CameraOptions::default() };
            let update = mln_engine::camera::CameraUpdate { camera, ..mln_engine::camera::CameraUpdate::default() };
            wait(map.update_camera(update));
        }),
        ("new data for the labels", |map| set_points(map, "labels", &[(4.5, 5.0), (6.0, 5.0)])),
        ("a symbol layer's icon-translate", |map| {
            wait(map.set_style_layer_property("labels", "icon-translate", "[10, 0]"));
        }),
        ("a symbol layer's visibility", |map| {
            wait(map.set_style_layer_property("more-labels", "visibility", r#""none""#));
        }),
        ("the symbol layers' order", |map| {
            wait(map.move_style_layer("more-labels", Some("labels".to_owned())));
        }),
    ]
}

#[test]
fn a_change_that_moves_no_label_asks_for_no_frames() {
    for (name, change) in changes_that_move_no_label() {
        let map = labels_and_vehicles();
        settle_placed(&map);
        change(&map.map);
        let frames = settle(&map);
        assert!(!frames.iter().any(|status| status.placement_changed), "{name}: a placement ran");
        assert!(!frames.iter().any(|status| status.needs_repaint), "{name}: the map asked for frames");
    }
}

#[test]
fn a_change_to_what_placement_reads_places_again() {
    for (name, change) in changes_to_placement_inputs() {
        let map = labels_and_vehicles();
        settle_placed(&map);
        change(&map.map);
        let frames = settle(&map);
        assert!(
            frames.iter().any(|status| status.placement_changed),
            "{name}: no placement ran in {} frames",
            frames.len()
        );
    }
}

/// The tiles of zoom 4 stay after the map shows zoom 5's, held while their
/// symbols fade out over the next two placements, which run although
/// nothing placement reads changes in between.
#[test]
fn tiles_held_for_their_fade_let_the_map_go_idle() {
    let map = labels_and_vehicles();
    settle_placed(&map);
    map.jump(CameraOptions { zoom: Some(5.0), ..CameraOptions::default() });
    let frames = settle(&map);
    let placements = frames.iter().filter(|status| status.placement_changed).count();
    assert!(placements >= 2, "{placements} placements");
}

/// With no symbols to place, each frame commits the empty placement: the
/// tiles of zoom 4 are held for the two frames after the map shows zoom
/// 5's, and the map goes idle.
#[test]
fn tiles_held_with_no_symbols_to_place_let_the_map_go_idle() {
    let map = labels_and_vehicles();
    for layer in ["labels", "more-labels"] {
        wait(map.map.set_style_layer_property(layer, "visibility", r#""none""#));
    }
    settle(&map);
    map.jump(CameraOptions { zoom: Some(5.0), ..CameraOptions::default() });
    settle(&map);
}
