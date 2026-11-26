// Symbol placement in a continuous map runs again when something it places
// from changes, and only then: a change that moves no label asks for no
// frames beyond those that show it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use feature::Value;
use geojson::GeoJson;
use mln_engine::RenderMode;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{Map, MapMode, MapOptions, RenderFrameStatus};
use mln_engine::storage::stub_file_source::{ResponseType, StubFileSource};
use mln_headless::HeadlessFrontend;
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::feature::FeatureState;
use mln_style::size::Size;
use mln_util::pool::Pool;
use serde_json::json;

use crate::gtest_api::style_image;
use crate::gtest_map::layer_property;
use crate::support::*;

/// The time between frames: a 60 Hz display's.
const FRAME: Duration = Duration::from_millis(16);

/// A continuous map of icons, from two sources, over circles from a third,
/// whose frames the test draws at the times it chooses, a frame apart once
/// the map has loaded. Its background work runs on the test's thread,
/// before each frame. Paint properties don't transition, so only placement
/// and its fades keep the map drawing.
struct ContinuousMap {
    _events: Observing,
    run_loop: TestLoop,
    pool: Pool,
    frontend: Rc<RefCell<HeadlessFrontend>>,
    observer: Rc<StubMapObserver>,
    map: Rc<RefCell<Map>>,
    start: Instant,
    /// The time of the next frame, from `start`.
    time: Cell<Duration>,
    /// The map's update generation when the last frame drew.
    drawn: Cell<u64>,
}

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

fn style() -> String {
    let source = |coordinates: &[(f64, f64)]| json!({"type": "geojson", "data": serde_json::to_value(points(coordinates)).expect("JSON")});
    json!({
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
    })
    .to_string()
}

impl ContinuousMap {
    fn new() -> Self {
        let run_loop = TestLoop::new();
        let pool = Pool::local();
        let mut frontend =
            HeadlessFrontend::new(Size::new(256, 256), 1.0, run_loop.queue(), &pool).expect("a headless frontend");
        let file_source = StubFileSource::on_queue(run_loop.queue(), ResponseType::Asynchronous);
        let observer = Rc::new(StubMapObserver::default());
        let mut map = Map::with_file_source(
            file_source,
            MapOptions { map_mode: MapMode::Continuous, size: Size::new(256, 256), ..Default::default() },
            &pool,
        )
        .unwrap();
        frontend.attach(&mut map);
        map.load_style_json(&style());
        let marker = PremultipliedImage::from_vec(Size::new(16, 16), vec![255; 16 * 16 * 4]);
        map.edit_style(|style| style.apply(Edit::AddImage(style_image("marker", marker, 1.0)))).unwrap();
        map.jump_to(&CameraOptions {
            center: Some(mln_style::geo::LatLng::new(5.0, 5.0).unwrap()),
            zoom: Some(4.0),
            ..Default::default()
        });
        let map = Rc::new(RefCell::new(map));
        let frontend = Rc::new(RefCell::new(frontend));
        let events = observer.observe(&run_loop, &map, None);
        ContinuousMap {
            _events: events,
            run_loop,
            pool,
            frontend,
            observer,
            map,
            start: Instant::now(),
            time: Cell::new(Duration::ZERO),
            drawn: Cell::new(0),
        }
    }

    /// Runs the background work, and the map's handling of what it
    /// reported, until none is left.
    fn wait_for_workers(&self) {
        loop {
            self.pool.run_all();
            let ran = self.run_loop.run_once();
            let reported = {
                let mut map = self.map.borrow_mut();
                let reported = map.has_reports();
                if reported {
                    map.process_events();
                }
                reported
            };
            self.observer.dispatch_events(&self.map);
            if !ran && !reported {
                return;
            }
        }
    }

    /// Draws the map's latest update at the current time, and returns how
    /// the frame went.
    fn frame(&self) -> RenderFrameStatus {
        self.wait_for_workers();
        let status = Rc::new(RefCell::new(None));
        {
            let status = Rc::clone(&status);
            self.observer.did_finish_rendering_frame_callback.set(move |value| *status.borrow_mut() = Some(value));
        }
        let (update, generation) = {
            let map = self.map.borrow();
            (map.update_reader().latest().expect("update parameters"), map.update_generation())
        };
        self.drawn.set(generation);
        let mut parameters = (*update.parameters).clone();
        parameters.time_point = Some(self.start + self.time.get());
        self.frontend.borrow_mut().draw(&Arc::new(parameters));
        self.observer.dispatch_events(&self.map);
        self.observer.did_finish_rendering_frame_callback.clear();
        status.borrow_mut().take().expect("rendered a frame")
    }

    /// Draws frames while the map loads, asks for them or publishes new
    /// updates, until it shows its view complete. The clock stands still
    /// while the map loads, and skips to the frame at or after the time the
    /// map changes next on its own ([`Map::repaint_at`]). Returns the
    /// frames.
    fn settle(&self) -> Vec<RenderFrameStatus> {
        let mut frames = Vec::new();
        loop {
            let status = self.frame();
            let loaded = status.mode == RenderMode::Full;
            let needs_repaint = status.needs_repaint;
            frames.push(status);
            if loaded {
                self.time.set(self.time.get() + FRAME);
            }
            self.wait_for_workers();
            let mut map = self.map.borrow_mut();
            let updated = map.update_generation() != self.drawn.get();
            if loaded && !updated {
                if let Some(at) = map.repaint_at() {
                    while self.start + self.time.get() < at {
                        self.time.set(self.time.get() + FRAME);
                    }
                    map.trigger_repaint();
                } else if !needs_repaint {
                    return frames;
                }
            }
            drop(map);
            assert!(frames.len() < 600, "the map is still drawing after {} frames", frames.len());
        }
    }

    /// Settles the map, which it does at a placement: a change made next
    /// comes while that placement is recent.
    fn settle_after_a_placement(&self) {
        let frames = self.settle();
        assert!(frames.last().expect("a frame").placement_changed, "the map settles at a placement");
    }
}

/// A change to a map, by name.
type Change = (&'static str, fn(&mut Map));

/// Changes that no symbol's placement reads.
fn changes_that_move_no_label() -> Vec<Change> {
    vec![
        ("new data for the circles", |map| set_geojson(map, "vehicles", points(&[(5.5, 6.0), (4.5, 4.0)]))),
        ("a circle paint property", |map| {
            map.edit_style(|style| style.apply(layer_property("vehicles", "circle-radius", json!(8)))).unwrap();
        }),
        ("a symbol paint property placement doesn't read", |map| {
            map.edit_style(|style| style.apply(layer_property("labels", "icon-opacity", json!(0.5)))).unwrap();
        }),
        ("a circle's feature state", |map| {
            let mut state = FeatureState::new();
            state.insert("hover".into(), Value::Bool(true));
            map.set_feature_state("vehicles", None, "0", state);
        }),
    ]
}

/// Changes to what placement places from.
fn changes_to_placement_inputs() -> Vec<Change> {
    vec![
        ("the camera", |map| map.jump_to(&CameraOptions { bearing: Some(30.0), ..Default::default() })),
        ("new data for the labels", |map| set_geojson(map, "labels", points(&[(4.5, 5.0), (6.0, 5.0)]))),
        ("a symbol layer's icon-translate", |map| {
            map.edit_style(|style| style.apply(layer_property("labels", "icon-translate", json!([10, 0])))).unwrap();
        }),
        ("a symbol layer's visibility", |map| {
            map.edit_style(|style| style.apply(layer_property("more-labels", "visibility", json!("none")))).unwrap();
        }),
        ("the symbol layers' order", |map| {
            map.edit_style(|style| {
                style.apply(Edit::MoveLayer { id: "more-labels".to_owned(), before: Some("labels".to_owned()) })
            })
            .unwrap();
        }),
    ]
}

#[test]
fn a_change_that_moves_no_label_asks_for_no_frames() {
    for (name, change) in changes_that_move_no_label() {
        let test = ContinuousMap::new();
        test.settle_after_a_placement();
        change(&mut test.map.borrow_mut());
        let frames = test.settle();
        assert!(!frames.iter().any(|status| status.placement_changed), "{name}: a placement ran");
        assert!(!frames.iter().any(|status| status.needs_repaint), "{name}: the map asked for frames");
    }
}

#[test]
fn a_change_to_what_placement_reads_places_again() {
    for (name, change) in changes_to_placement_inputs() {
        let test = ContinuousMap::new();
        test.settle_after_a_placement();
        change(&mut test.map.borrow_mut());
        let frames = test.settle();
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
    let test = ContinuousMap::new();
    test.settle_after_a_placement();
    test.map.borrow_mut().jump_to(&CameraOptions { zoom: Some(5.0), ..Default::default() });
    let frames = test.settle();
    let placements = frames.iter().filter(|status| status.placement_changed).count();
    assert!(placements >= 2, "{placements} placements");
}

/// With no symbols to place, each frame commits the empty placement: the
/// tiles of zoom 4 are held for the two frames after the map shows zoom
/// 5's, and the map goes idle.
#[test]
fn tiles_held_with_no_symbols_to_place_let_the_map_go_idle() {
    let test = ContinuousMap::new();
    for layer in ["labels", "more-labels"] {
        test.map
            .borrow_mut()
            .edit_style(|style| style.apply(layer_property(layer, "visibility", json!("none"))))
            .unwrap();
    }
    test.settle();
    test.map.borrow_mut().jump_to(&CameraOptions { zoom: Some(5.0), ..Default::default() });
    test.settle();
}

/// A camera move that moves no label waits for the next placement without
/// drawing: one frame shows the move, and one at the placement's time finds
/// nothing to change.
#[test]
fn a_camera_move_that_moves_no_label_draws_twice() {
    let test = ContinuousMap::new();
    test.settle_after_a_placement();
    let nudged = mln_style::geo::LatLng::new(5.0, 5.001).unwrap();
    test.map.borrow_mut().jump_to(&CameraOptions { center: Some(nudged), ..Default::default() });
    let frames = test.settle();
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert!(!frames[0].placement_changed && frames[1].placement_changed);
}
