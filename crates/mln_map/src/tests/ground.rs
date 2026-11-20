//! How the camera stands on the terrain its renderer drew, and how stills
//! and idle wait for it to settle.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use mln_map_types::Elevation;
use mln_renderer::{RenderMode, RendererEvent};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_util::geo::LatLng;
use mln_util::queue::Queue;

use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, Map, MapEvent, MapMode, MapOptions};

const STYLE: &str = r#"{"version": 8, "center": [10, 20], "zoom": 3, "sources": {}, "layers": []}"#;

/// Ground of the same height everywhere, `None` where the heights are still
/// loading.
#[derive(Debug)]
struct Flat {
    height: Option<f64>,
    min: f64,
}

impl Elevation for Flat {
    fn at(&self, _: LatLng) -> Option<f64> {
        self.height
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(self.min)
    }
}

/// Ground whose height alternates between 100 m and 200 m with each read,
/// on which a camera never settles.
#[derive(Debug, Default)]
struct Restless {
    reads: AtomicU32,
}

impl Elevation for Restless {
    fn at(&self, _: LatLng) -> Option<f64> {
        Some(if self.reads.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) { 100.0 } else { 200.0 })
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(100.0)
    }
}

fn flat(height: Option<f64>, min: f64) -> RendererEvent {
    RendererEvent::TerrainElevation(Some(Arc::new(Flat { height, min })))
}

fn map(mode: MapMode, run_loop: &Queue<()>) -> Map {
    let file_source = StubFileSource::on_queue(run_loop, ResponseType::Asynchronous);
    let options = MapOptions { map_mode: mode, ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &crate::tests::pool()).expect("map");
    map.load_style_json(STYLE);
    map.process_events();
    map
}

/// Hands `event` to the map as its renderer would, and has the map handle it.
fn report(map: &mut Map, event: RendererEvent) {
    map.renderer_events().push(event);
    map.process_events();
}

/// The center's altitude and the floor of the view's planes.
fn ground(map: &Map) -> (f64, Option<f64>) {
    let view = map.transform_state().view();
    (view.center_altitude(), view.terrain_min_elevation())
}

/// The ground generation of the map's latest update.
fn ground_generation(map: &Map) -> u64 {
    map.update_reader().latest().expect("an update").parameters.ground_generation
}

fn frame(still_image_request_id: u64, ground_generation: u64) -> RendererEvent {
    RendererEvent::RenderFrameFinished {
        mode: RenderMode::Full,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id,
        ground_generation,
        stats: Arc::default(),
    }
}

fn camera_events(events: &[MapEvent]) -> usize {
    events.iter().filter(|event| matches!(event, MapEvent::CameraWillChange(_) | MapEvent::CameraDidChange(_))).count()
}

/// A camera at rest stands on the ground under its center, which the
/// update that the move publishes shows, without camera events. One that an
/// animation moves keeps its altitude, though the planes fit the terrain,
/// until it comes to rest.
#[test]
fn a_camera_at_rest_stands_on_the_ground_and_a_moving_one_holds() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    map.drain_events();

    let updates = map.update_generation();
    report(&mut map, flat(Some(500.0), 300.0));
    assert_eq!((500.0, Some(300.0)), ground(&map));
    assert_eq!(updates + 1, map.update_generation(), "the move's update");
    assert_eq!(1, ground_generation(&map));
    assert_eq!(0, camera_events(&map.drain_events()));

    // Ground that stays the same moves nothing.
    report(&mut map, flat(Some(500.0), 300.0));
    assert_eq!(1, ground_generation(&map));

    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(10.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(60)), ..Default::default() },
        ..Default::default()
    });
    report(&mut map, flat(Some(800.0), 700.0));
    let (altitude, floor) = ground(&map);
    assert!((altitude - 500.0).abs() < 1e-6, "{altitude}");
    assert_eq!(Some(700.0), floor);

    map.cancel_transitions();
    let (altitude, _) = ground(&map);
    assert!((altitude - 800.0).abs() < 1e-6, "{altitude}");
}

/// With its center unclamped, the camera keeps the altitude it was given,
/// and the planes still fit the terrain.
#[test]
fn an_unclamped_center_keeps_its_altitude() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { center_altitude: Some(2000.0), ..Default::default() });
    report(&mut map, flat(Some(500.0), 300.0));
    let (altitude, floor) = ground(&map);
    assert!((altitude - 2000.0).abs() < 1e-6, "{altitude}");
    assert_eq!(Some(300.0), floor);

    // Clamping it again stands it on the ground.
    map.set_center_clamped_to_ground(true);
    assert_eq!((500.0, Some(300.0)), ground(&map));
}

/// Where the ground is still loading, the center keeps its altitude.
#[test]
fn loading_ground_keeps_the_altitude() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    report(&mut map, flat(Some(500.0), 300.0));
    report(&mut map, flat(None, 0.0));
    assert_eq!((500.0, Some(0.0)), ground(&map));
}

/// Once terrain stops drawing, the planes fit the flat map and a clamped
/// center goes back to sea level, once: an altitude given after stays.
#[test]
fn the_camera_returns_to_sea_level_once_terrain_stops() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    report(&mut map, flat(Some(500.0), 300.0));
    let flat_view = {
        let mut state = map.transform_state().clone();
        state.set_terrain_min_elevation(None);
        state.set_center_altitude(0.0);
        state.view().clone()
    };

    report(&mut map, RendererEvent::TerrainElevation(None));
    assert_eq!((0.0, None), ground(&map));
    assert_eq!(flat_view, *map.transform_state().view());
    assert_eq!(2, ground_generation(&map));

    map.jump_to(&CameraOptions { center_altitude: Some(100.0), ..Default::default() });
    map.trigger_repaint();
    let (altitude, _) = ground(&map);
    assert!((altitude - 100.0).abs() < 1e-6, "{altitude}");
    assert_eq!(2, ground_generation(&map));
}

/// A complete frame drawn before the terrain moved the camera neither
/// completes a still image nor leaves a continuous map idle: the move's
/// update draws another, which does.
#[test]
fn a_frame_of_an_earlier_ground_ends_no_still_and_no_idle() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    report(&mut map, flat(Some(500.0), 300.0));
    report(&mut map, frame(1, 0));
    assert_eq!(None, map.take_still_image());
    let latest = ground_generation(&map);
    report(&mut map, frame(1, latest));
    assert_eq!(Some(Ok(())), map.take_still_image());

    let mut map = self::map(MapMode::Continuous, &run_loop);
    report(&mut map, flat(Some(500.0), 300.0));
    map.drain_events();
    let idle = |events: Vec<MapEvent>| events.iter().any(|event| matches!(event, MapEvent::Idle));
    report(&mut map, frame(0, 0));
    assert!(!idle(map.drain_events()));
    let latest = ground_generation(&map);
    report(&mut map, frame(0, latest));
    assert!(idle(map.drain_events()));
}

/// A camera whose ground changes with each move stops following it after
/// eight moves in a still image, which the next complete frame then ends.
#[test]
fn a_still_image_stops_following_ground_that_never_settles() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    let restless: Arc<dyn Elevation> = Arc::new(Restless::default());
    for _ in 0..20 {
        report(&mut map, RendererEvent::TerrainElevation(Some(Arc::clone(&restless))));
        let earlier = ground_generation(&map).saturating_sub(1);
        report(&mut map, frame(1, earlier));
        assert_eq!(None, map.take_still_image());
    }
    assert_eq!(8, ground_generation(&map));
    report(&mut map, frame(1, 8));
    assert_eq!(Some(Ok(())), map.take_still_image());

    // The next still image follows the ground again.
    map.render_still().unwrap();
    report(&mut map, RendererEvent::TerrainElevation(Some(restless)));
    assert!(ground_generation(&map) > 8);
}
