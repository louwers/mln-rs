//! How the camera stands on the terrain its renderer drew, and how the
//! map's updates carry that ground back to the renderer, which completes a
//! frame only of an update whose camera stands as it would on its newest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use mln_map_types::Elevation;
use mln_renderer::{RenderMode, RendererEvent};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::Edit;
use mln_util::geo::LatLng;
use mln_util::queue::Queue;

use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, Map, MapEvent, MapMode, MapOptions};

const STYLE: &str = r#"{
    "version": 8,
    "center": [10, 20],
    "zoom": 3,
    "terrain": {"source": "dem"},
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"]}},
    "layers": []
}"#;

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

fn flat(height: Option<f64>, min: f64) -> Arc<dyn Elevation> {
    Arc::new(Flat { height, min })
}

fn ground_event(elevation: &Arc<dyn Elevation>) -> RendererEvent {
    RendererEvent::TerrainElevation(Some(Arc::clone(elevation)))
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

/// Whether the map's latest update carries `elevation` as its ground.
fn carries(map: &Map, elevation: Option<&Arc<dyn Elevation>>) -> bool {
    let update = map.update_reader().latest().expect("an update");
    match (update.parameters.elevation.as_ref(), elevation) {
        (Some(carried), Some(elevation)) => std::ptr::addr_eq(Arc::as_ptr(carried), Arc::as_ptr(elevation)),
        (carried, elevation) => carried.is_none() && elevation.is_none(),
    }
}

fn frame(mode: RenderMode, still_image_request_id: u64) -> RendererEvent {
    RendererEvent::RenderFrameFinished {
        mode,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id,
        stats: Arc::default(),
    }
}

fn camera_events(events: &[MapEvent]) -> usize {
    events.iter().filter(|event| matches!(event, MapEvent::CameraWillChange(_) | MapEvent::CameraDidChange(_))).count()
}

/// A camera at rest stands on the ground under its center, which the
/// update that answers the ground shows, without camera events. One that an
/// animation moves keeps its altitude, though the planes fit the terrain,
/// until it comes to rest.
#[test]
fn a_camera_at_rest_stands_on_the_ground_and_a_moving_one_holds() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    map.drain_events();

    let updates = map.update_generation();
    let ground = flat(Some(500.0), 300.0);
    report(&mut map, ground_event(&ground));
    assert_eq!((500.0, Some(300.0)), self::ground(&map));
    assert_eq!(updates + 1, map.update_generation(), "the answer's update");
    assert!(carries(&map, Some(&ground)));
    assert_eq!(0, camera_events(&map.drain_events()));

    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(10.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(60)), ..Default::default() },
        ..Default::default()
    });
    report(&mut map, ground_event(&flat(Some(800.0), 700.0)));
    let (altitude, floor) = self::ground(&map);
    assert!((altitude - 500.0).abs() < 1e-6, "{altitude}");
    assert_eq!(Some(700.0), floor);

    map.cancel_transitions();
    let (altitude, _) = self::ground(&map);
    assert!((altitude - 800.0).abs() < 1e-6, "{altitude}");
}

/// The map answers each ground the renderer tells of with an update that
/// carries it, though the camera doesn't move: the renderer completes a
/// frame only of an update that stands the camera as it does.
#[test]
fn each_ground_gets_an_update_that_carries_it() {
    let run_loop = Queue::<()>::new();
    for mode in [MapMode::Continuous, MapMode::Static] {
        let mut map = map(mode, &run_loop);
        if mode == MapMode::Static {
            map.render_still().unwrap();
        }
        let first = flat(Some(500.0), 300.0);
        report(&mut map, ground_event(&first));
        assert!(carries(&map, Some(&first)), "{mode:?}");

        let updates = map.update_generation();
        let same = flat(Some(500.0), 300.0);
        report(&mut map, ground_event(&same));
        assert_eq!((500.0, Some(300.0)), ground(&map), "{mode:?}: nothing moved");
        assert_eq!(updates + 1, map.update_generation(), "{mode:?}");
        assert!(carries(&map, Some(&same)), "{mode:?}");

        report(&mut map, RendererEvent::TerrainElevation(None));
        assert!(carries(&map, None), "{mode:?}");
    }
}

/// With its center unclamped, the camera keeps the altitude it was given,
/// and the planes still fit the terrain.
#[test]
fn an_unclamped_center_keeps_its_altitude() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { center_altitude: Some(2000.0), ..Default::default() });
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
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
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    report(&mut map, ground_event(&flat(None, 0.0)));
    assert_eq!((500.0, Some(0.0)), ground(&map));
}

/// Once terrain stops drawing, the planes fit the flat map and a clamped
/// center goes back to sea level, once: an altitude given after stays.
#[test]
fn the_camera_returns_to_sea_level_once_terrain_stops() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    let flat_view = {
        let mut state = map.transform_state().clone();
        state.set_terrain_min_elevation(None);
        state.set_center_altitude(0.0);
        state.view().clone()
    };

    report(&mut map, RendererEvent::TerrainElevation(None));
    assert_eq!((0.0, None), ground(&map));
    assert_eq!(flat_view, *map.transform_state().view());

    map.jump_to(&CameraOptions { center_altitude: Some(100.0), ..Default::default() });
    map.trigger_repaint();
    let (altitude, _) = ground(&map);
    assert!((altitude - 100.0).abs() < 1e-6, "{altitude}");
}

/// A still map that waits for no still image publishes no update, but its
/// camera stands on the ground all the same, and the next still image's
/// first update carries it.
#[test]
fn a_still_map_between_images_stands_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    let ground_ = flat(Some(500.0), 300.0);
    report(&mut map, ground_event(&ground_));
    assert_eq!((500.0, Some(300.0)), ground(&map));
    map.render_still().unwrap();
    assert!(carries(&map, Some(&ground_)));
    report(&mut map, RendererEvent::TerrainElevation(None));
    assert_eq!((0.0, None), ground(&map));
}

/// Only a complete frame ends a still image or leaves a continuous map
/// idle: the renderer reports one of an update that carries an earlier
/// ground as partial.
#[test]
fn a_partial_frame_ends_no_still_and_no_idle() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    report(&mut map, frame(RenderMode::Partial, 1));
    assert_eq!(None, map.take_still_image());
    report(&mut map, frame(RenderMode::Full, 1));
    assert_eq!(Some(Ok(())), map.take_still_image());

    let mut map = self::map(MapMode::Continuous, &run_loop);
    map.drain_events();
    let idle = |events: Vec<MapEvent>| events.iter().any(|event| matches!(event, MapEvent::Idle));
    report(&mut map, frame(RenderMode::Partial, 0));
    assert!(!idle(map.drain_events()));
    report(&mut map, frame(RenderMode::Full, 0));
    assert!(idle(map.drain_events()));
}

/// A camera whose ground changes with each move stops following it after
/// eight moves in a still image, though the map still answers each ground
/// with an update that carries it, so the renderer completes the image.
#[test]
fn a_still_image_stops_following_ground_that_never_settles() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    let restless: Arc<dyn Elevation> = Arc::new(Restless::default());
    let mut altitudes = Vec::new();
    for _ in 0..20 {
        let updates = map.update_generation();
        report(&mut map, ground_event(&restless));
        assert_eq!(updates + 1, map.update_generation(), "an answer to each");
        assert!(carries(&map, Some(&restless)));
        altitudes.push(ground(&map).0);
    }
    let mut moves = [100.0, 200.0].repeat(4);
    moves.extend([200.0; 12]);
    assert_eq!(moves, altitudes);
    report(&mut map, frame(RenderMode::Full, 1));
    assert_eq!(Some(Ok(())), map.take_still_image());

    // The next still image follows the ground again.
    map.render_still().unwrap();
    assert_eq!(100.0, ground(&map).0);
}

/// A style that drops its terrain while a still image waits takes the
/// camera back to the flat map at once, before the renderer has drawn the
/// style without terrain, and its updates carry no ground.
#[test]
fn a_style_without_terrain_lets_go_of_the_ground() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    let ground_ = flat(Some(500.0), 300.0);
    report(&mut map, ground_event(&ground_));
    assert_eq!((500.0, Some(300.0)), ground(&map));

    map.edit_style(|style| style.apply(Edit::SetTerrain(None))).unwrap();
    assert_eq!((0.0, None), ground(&map));
    assert!(carries(&map, None));
    // A ground the renderer sent before it drew the flat map doesn't ground
    // the camera again.
    report(&mut map, ground_event(&ground_));
    assert_eq!((0.0, None), ground(&map));
    assert!(carries(&map, None));
}

/// A still image that used up its moves on ground that never settles still
/// goes back to the flat map once terrain stops drawing.
#[test]
fn the_camera_returns_to_sea_level_after_a_still_image_used_its_moves() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    let restless: Arc<dyn Elevation> = Arc::new(Restless::default());
    for _ in 0..10 {
        report(&mut map, ground_event(&restless));
    }
    assert!(ground(&map).0 > 0.0);
    report(&mut map, RendererEvent::TerrainElevation(None));
    assert_eq!((0.0, None), ground(&map));
    assert!(carries(&map, None));
}

/// An animation of the pitch alone, or of the altitude alone, holds the
/// altitude as any animation does, until it ends.
#[test]
fn a_pitch_or_altitude_animation_holds_the_altitude() {
    for camera in [
        CameraOptions { pitch: Some(40.0), ..Default::default() },
        CameraOptions { center_altitude: Some(2000.0), ..Default::default() },
    ] {
        let run_loop = Queue::<()>::new();
        let mut map = map(MapMode::Continuous, &run_loop);
        report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera,
            animation: AnimationOptions { duration: Some(Duration::from_secs(60)), ..Default::default() },
            ..Default::default()
        });
        map.trigger_repaint();
        let held = ground(&map).0;
        report(&mut map, ground_event(&flat(Some(800.0), 700.0)));
        let (altitude, floor) = ground(&map);
        assert!((altitude - held).abs() < 1e-6 && (altitude - 800.0).abs() > 1.0, "{camera:?}: {altitude}");
        assert_eq!(Some(700.0), floor);

        map.cancel_transitions();
        let (altitude, _) = ground(&map);
        assert!((altitude - 800.0).abs() < 1e-6, "{camera:?}: {altitude}");
    }
}
