//! How the camera stands on the terrain its renderer drew, and how the
//! map's updates carry that ground back to the renderer, which completes a
//! frame only of an update whose camera stands as it would on its newest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use glam::DVec2;
use mln_style::Edit;
use mln_style::geo::{LatLng, WrapMode};

use crate::camera::{AnimationOptions, BoundOptions, CameraOptions, CameraUpdate, CameraUpdateMode, Elevation};
use crate::exec::queue::Queue;
use crate::map::{Map, MapEvent, MapMode, MapOptions};
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::{RenderMode, RendererEvent};

pub(super) const STYLE: &str = r#"{
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

    fn surface_at(&self, _: DVec2) -> Option<f64> {
        Some(self.height.unwrap_or(0.0))
    }

    fn range(&self) -> Option<(f64, f64)> {
        let height = self.height.unwrap_or(0.0);
        Some((height, height))
    }
}

/// Sea level, with a plateau `height` meters high south of the world's `y`
/// (Web Mercator at zoom 0), which only the surface shows.
#[derive(Debug)]
pub(super) struct Plateau {
    pub(super) y: f64,
    pub(super) height: f64,
}

impl Elevation for Plateau {
    fn at(&self, _: LatLng) -> Option<f64> {
        Some(0.0)
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(0.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(if world.y > self.y { self.height } else { 0.0 })
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((self.height.min(0.0), self.height.max(0.0)))
    }
}

/// Ground 500 m high with a floor of 300 m at `center`, as [`Flat`] gives,
/// and far higher everywhere else, which counts the reads of anything but
/// the height and floor at `center`.
#[derive(Debug)]
struct CenterOnly {
    center: LatLng,
    reads_elsewhere: AtomicU32,
}

impl CenterOnly {
    fn read(&self, location: LatLng, at_center: f64) -> f64 {
        if location == self.center {
            return at_center;
        }
        self.reads_elsewhere.fetch_add(1, Ordering::SeqCst);
        100_000.0
    }
}

impl Elevation for CenterOnly {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(self.read(location, 500.0))
    }

    fn min_at(&self, location: LatLng) -> Option<f64> {
        Some(self.read(location, 300.0))
    }

    fn surface_at(&self, _: DVec2) -> Option<f64> {
        self.reads_elsewhere.fetch_add(1, Ordering::SeqCst);
        Some(100_000.0)
    }

    fn range(&self) -> Option<(f64, f64)> {
        self.reads_elsewhere.fetch_add(1, Ordering::SeqCst);
        Some((100_000.0, 100_000.0))
    }
}

/// Ground that rises `rise` meters for each unit of Web Mercator x at zoom
/// 0 east of `x`, where it is `height` meters high, with a floor of
/// `height` under every center.
#[derive(Debug)]
pub(super) struct Slope {
    pub(super) x: f64,
    pub(super) height: f64,
    pub(super) rise: f64,
}

impl Slope {
    fn height(&self, world: DVec2) -> f64 {
        self.height + self.rise * (world.x.rem_euclid(1.0) - self.x)
    }
}

impl Elevation for Slope {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(self.height(world(&location.wrapped())))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(self.height)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(self.height(world))
    }

    fn range(&self) -> Option<(f64, f64)> {
        let (west, east) = (self.height - self.rise * self.x, self.height + self.rise * (1.0 - self.x));
        Some((west.min(east), west.max(east)))
    }
}

pub(super) fn flat(height: Option<f64>, min: f64) -> Arc<dyn Elevation> {
    Arc::new(Flat { height, min })
}

/// The `n`th of a run of grounds that never settles: each is new, 100 m
/// high for an even `n` and 200 m for an odd one.
fn restless(n: usize) -> Arc<dyn Elevation> {
    flat(Some(if n.is_multiple_of(2) { 100.0 } else { 200.0 }), 100.0)
}

pub(super) fn ground_event(elevation: &Arc<dyn Elevation>) -> RendererEvent {
    RendererEvent::TerrainElevation(Some(Arc::clone(elevation)))
}

fn map(mode: MapMode, run_loop: &Queue<()>) -> Map {
    let file_source = StubFileSource::on_queue(run_loop, ResponseType::Asynchronous);
    let options = MapOptions { map_mode: mode, ..Default::default() };
    let mut map =
        Map::with_file_source(file_source, options, crate::tests::registry(), &crate::map::tests::pool()).expect("map");
    map.load_style_json(STYLE);
    map.process_events();
    map
}

/// A 512 by 512 map of `style` that may pitch up to 180 degrees.
pub(super) fn pitchable(style: &str, run_loop: &Queue<()>) -> Map {
    let file_source = StubFileSource::on_queue(run_loop, ResponseType::Asynchronous);
    let options = MapOptions { size: mln_style::size::Size::new(512, 512), ..Default::default() };
    let mut map =
        Map::with_file_source(file_source, options, crate::tests::registry(), &crate::map::tests::pool()).expect("map");
    map.load_style_json(style);
    map.process_events();
    map.set_bounds(&BoundOptions { max_pitch: Some(180.0), ..Default::default() }).unwrap();
    map
}

/// Where `location` is in Web Mercator at zoom 0.
pub(super) fn world(location: &LatLng) -> DVec2 {
    let point = crate::geo::projection::Projection::project(location, 1.0 / mln_style::constants::TILE_SIZE_D);
    DVec2::new(point.x, point.y)
}

pub(super) fn assert_near(expected: f64, actual: f64, tolerance: f64, what: &str) {
    assert!((expected - actual).abs() <= tolerance, "{what}: {actual}, not {expected}");
}

/// Hands `event` to the map as its renderer would, and has the map handle it.
pub(super) fn report(map: &mut Map, event: RendererEvent) {
    map.renderer_events().push(event);
    map.process_events();
}

/// The center's altitude and the floor of the view's planes.
pub(super) fn ground(map: &Map) -> (f64, Option<f64>) {
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

/// The `update_id` of the map's latest update.
fn latest_update(map: &Map) -> u64 {
    map.update_reader().latest().map_or(0, |update| update.parameters.update_id)
}

/// The report of a frame in `mode` of the update `update_id`, of the still
/// image `still_image_request_id`, or of none with 0.
fn frame(mode: RenderMode, still_image_request_id: u64, update_id: u64) -> RendererEvent {
    RendererEvent::RenderFrameFinished {
        mode,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id,
        update_id,
        stats: Arc::default(),
        error: None,
    }
}

/// Reports a frame of the map's latest update in `mode`, of the still image
/// `still_image_request_id`, or of none with 0.
fn report_frame(map: &mut Map, mode: RenderMode, still_image_request_id: u64) {
    let update_id = latest_update(map);
    report(map, frame(mode, still_image_request_id, update_id));
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

/// The terrain moves a camera at rest without camera events, whatever moves
/// it: ground that loads, changes or stops drawing under it, the clamp
/// turned on, and the style dropping its terrain. Hosts see those moves in
/// the snapshot.
#[test]
fn ground_at_rest_emits_no_camera_events() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    map.drain_events();
    let silent = |map: &mut Map, altitude: f64, what: &str| {
        assert_eq!(altitude, ground(map).0, "{what}");
        assert_eq!(0, camera_events(&map.drain_events()), "{what}");
    };

    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    silent(&mut map, 500.0, "ground that loads");
    report(&mut map, ground_event(&flat(Some(800.0), 700.0)));
    silent(&mut map, 800.0, "ground that changes");
    report(&mut map, ground_event(&flat(None, 0.0)));
    silent(&mut map, 800.0, "ground that loads again");
    map.set_center_clamped_to_ground(false);
    report(&mut map, ground_event(&flat(Some(200.0), 100.0)));
    silent(&mut map, 800.0, "unclamped ground");
    map.set_center_clamped_to_ground(true);
    silent(&mut map, 200.0, "the clamp turned on");
    report(&mut map, RendererEvent::TerrainElevation(None));
    silent(&mut map, 0.0, "ground that stops drawing");
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    silent(&mut map, 500.0, "ground that draws again");
    map.edit_style(|style| style.apply(Edit::SetTerrain(None))).unwrap();
    silent(&mut map, 0.0, "the style's terrain dropped");
}

/// The map answers each new ground the renderer tells of with an update
/// that carries it, though the camera doesn't move: the renderer completes
/// a frame only of an update that carries its ground. Ground the map
/// already holds, which the renderer tells of again for an update that was
/// in flight, gets no update.
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
        report(&mut map, ground_event(&same));
        assert_eq!(updates + 1, map.update_generation(), "{mode:?}: the ground it holds");

        report(&mut map, RendererEvent::TerrainElevation(None));
        assert!(carries(&map, None), "{mode:?}");
        report(&mut map, RendererEvent::TerrainElevation(None));
        assert_eq!(updates + 2, map.update_generation(), "{mode:?}: no ground again");
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

/// Where the renderer draws the ground flat, as while its DEM loads, the
/// center comes down to sea level: the camera stands on what is drawn.
#[test]
fn ground_drawn_flat_brings_the_center_to_sea_level() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    report(&mut map, ground_event(&flat(Some(0.0), 0.0)));
    assert_eq!((0.0, Some(0.0)), ground(&map));
}

/// Once terrain stops drawing, the planes fit the flat map and a clamped
/// center goes back to sea level, once: an altitude the camera takes while
/// the clamp is off stays once it is on again.
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

    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { center_altitude: Some(100.0), ..Default::default() });
    map.set_center_clamped_to_ground(true);
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
    report_frame(&mut map, RenderMode::Partial, 1);
    assert_eq!(None, map.take_still_image());
    report_frame(&mut map, RenderMode::Full, 1);
    assert_eq!(Some(Ok(())), map.take_still_image());

    let mut map = self::map(MapMode::Continuous, &run_loop);
    map.drain_events();
    let idle = |events: Vec<MapEvent>| events.iter().any(|event| matches!(event, MapEvent::Idle));
    report_frame(&mut map, RenderMode::Partial, 0);
    assert!(!idle(map.drain_events()));
    report_frame(&mut map, RenderMode::Full, 0);
    assert!(idle(map.drain_events()));
}

/// A continuous map goes idle only on a frame of its last update. A frame
/// that took in new ground, or a tile's data whose wake reaches the map
/// late, reports it before it finishes, and the map answers with an
/// update: the frame, complete as it is, leaves that update to draw, and
/// the update's frame leaves the map idle, once.
#[test]
fn a_continuous_map_goes_idle_only_on_a_frame_of_its_last_update() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    let idle = |map: &mut Map| map.drain_events().iter().filter(|event| matches!(event, MapEvent::Idle)).count();
    for (what, taken_in) in
        [("new ground", ground_event(&flat(Some(0.0), 0.0))), ("a late wake", RendererEvent::Invalidate)]
    {
        report_frame(&mut map, RenderMode::Full, 0);
        assert_eq!(1, idle(&mut map), "the map isn't idle before {what}");
        let drawn = latest_update(&map);
        map.renderer_events().push(taken_in);
        map.renderer_events().push(frame(RenderMode::Full, 0, drawn));
        map.process_events();
        assert_ne!(drawn, latest_update(&map), "the map answers {what} with an update");
        assert_eq!(0, idle(&mut map), "idle on a frame before the answer to {what}");
        let answer = latest_update(&map);
        report_frame(&mut map, RenderMode::Full, 0);
        assert_eq!(1, idle(&mut map), "the answer to {what} leaves the map idle");
        assert_eq!(answer, latest_update(&map), "the idle map asks for another frame after {what}");
    }
}

/// A camera whose ground changes with each move stops following it after
/// eight moves in a still image, though the map still answers each ground
/// with an update that carries it, so the renderer completes the image.
#[test]
fn a_still_image_stops_following_ground_that_never_settles() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Static, &run_loop);
    map.render_still().unwrap();
    let mut altitudes = Vec::new();
    for n in 0..20 {
        let updates = map.update_generation();
        let ground_ = restless(n);
        report(&mut map, ground_event(&ground_));
        assert_eq!(updates + 1, map.update_generation(), "an answer to each");
        assert!(carries(&map, Some(&ground_)));
        altitudes.push(ground(&map).0);
    }
    let mut moves = [100.0, 200.0].repeat(4);
    moves.extend([200.0; 12]);
    assert_eq!(moves, altitudes);
    report_frame(&mut map, RenderMode::Full, 1);
    assert_eq!(Some(Ok(())), map.take_still_image());

    // The next still image follows the ground again.
    map.render_still().unwrap();
    report(&mut map, ground_event(&restless(0)));
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
    for n in 0..10 {
        report(&mut map, ground_event(&restless(n)));
    }
    assert!(ground(&map).0 > 0.0);
    report(&mut map, RendererEvent::TerrainElevation(None));
    assert_eq!((0.0, None), ground(&map));
    assert!(carries(&map, None));
}

/// An animation of the pitch alone eases the altitude onto ground that
/// lands under the center, by its end, rather than jumping onto it. One of
/// the altitude alone, which the clamp decides, has nothing to animate: it
/// ends at once, with the center on the ground.
#[test]
fn a_pitch_animation_eases_onto_ground_that_lands() {
    let animate = |map: &mut Map, camera| {
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera,
            animation: AnimationOptions { duration: Some(Duration::from_secs(60)), ..Default::default() },
            ..Default::default()
        });
        map.trigger_repaint();
    };
    let run_loop = Queue::<()>::new();
    let mut map = map(MapMode::Continuous, &run_loop);
    report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
    animate(&mut map, CameraOptions { center_altitude: Some(2000.0), ..Default::default() });
    assert!(!map.snapshot().transition_in_progress, "the altitude alone");
    assert_eq!(500.0, ground(&map).0, "the altitude alone");

    animate(&mut map, CameraOptions { pitch: Some(40.0), ..Default::default() });
    report(&mut map, ground_event(&flat(Some(800.0), 700.0)));
    let (altitude, min) = ground(&map);
    assert_eq!(Some(700.0), min);
    assert!((500.0..501.0).contains(&altitude), "a minute's pitch heads for the ground slowly: {altitude}");

    map.cancel_transitions();
    let (altitude, _) = ground(&map);
    assert!((altitude - 800.0).abs() < 1e-6, "{altitude}");
}

/// A jump that would put the eye below sea level before the renderer drew
/// any ground raises it to sea level by pitch and zoom, with the eye where
/// it was across the ground and the center where the jump put it, as in GL
/// JS: the style's camera of `high-pitch/terrain-pitch95`.
#[test]
fn a_jump_below_sea_level_before_any_ground_looks_from_sea_level() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(STYLE, &run_loop);
    let center = LatLng::new(47.34487, 11.52517).unwrap();
    map.jump_to(&CameraOptions {
        center: Some(center),
        zoom: Some(13.0),
        pitch: Some(95.0),
        center_altitude: Some(0.0),
        ..Default::default()
    });
    let camera = map.transform_state().camera_options(None);
    assert_near(90.0, camera.pitch.unwrap(), 1e-9, "pitch");
    assert_near(13.0 + (1.0 / 95f64.to_radians().sin()).log2(), camera.zoom.unwrap(), 1e-9, "zoom");
    let moved = map.transform_state().view().center(WrapMode::Unwrapped);
    assert_near(center.latitude(), moved.latitude(), 1e-9, "latitude");
    assert_near(center.longitude(), moved.longitude(), 1e-9, "longitude");
    assert_eq!(0.0, camera.center_altitude.unwrap());
}

/// A jump that would put the eye, or the near plane, into raised ground is
/// raised by pitch and zoom until both clear it, keeping the center, its
/// altitude and the bearing. The eye would be about 2,300 m below the
/// plateau.
#[test]
fn a_jump_into_raised_ground_keeps_the_eye_above_it() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(STYLE, &run_loop);
    map.set_center_clamped_to_ground(false);
    let center = LatLng::new(20.0, 10.0).unwrap();
    let plateau = Plateau { y: world(&center).y + 1e-6, height: 4000.0 };
    let surface = |point: glam::DVec3| plateau.surface_at(point.truncate()).unwrap();
    report(
        &mut map,
        ground_event(&{
            let plateau: Arc<dyn Elevation> = Arc::new(Plateau { ..plateau });
            plateau
        }),
    );
    let jump = CameraOptions {
        center: Some(center),
        zoom: Some(14.0),
        pitch: Some(70.0),
        bearing: Some(30.0),
        center_altitude: Some(500.0),
        ..Default::default()
    };
    map.jump_to(&jump);

    let view = map.transform_state().view();
    let eye = view.eye_position();
    assert!(eye.z >= surface(eye), "the eye at {eye}");
    // Each raise turns the near plane down a little, so after GL JS's three
    // the near plane may stay a few millimeters short, out of sight.
    for point in view.near_plane_points() {
        assert!(point.z >= surface(point) - 0.01, "the near plane at {point}");
    }
    let camera = map.transform_state().camera_options(None);
    assert!(camera.pitch.unwrap() < 70.0 && camera.zoom.unwrap() < 14.0, "{camera:?}");
    let moved = view.center(WrapMode::Unwrapped);
    assert_near(center.latitude(), moved.latitude(), 1e-9, "latitude");
    assert_near(center.longitude(), moved.longitude(), 1e-9, "longitude");
    assert_eq!(500.0, camera.center_altitude.unwrap());
    assert_near(30.0, camera.bearing.unwrap(), 1e-9, "bearing");
}

/// Without terrain, a jump keeps the pitch it was given, though the eye is
/// below sea level.
#[test]
fn without_terrain_a_pitch_95_jump_stays_at_95() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(r#"{"version": 8, "sources": {}, "layers": []}"#, &run_loop);
    map.jump_to(&CameraOptions { zoom: Some(13.0), pitch: Some(95.0), ..Default::default() });
    assert!(map.transform_state().view().eye_position().z < 0.0);
    assert_near(95.0, map.transform_state().camera_options(None).pitch.unwrap(), 1e-9, "pitch");
    assert_eq!(Some(13.0), map.transform_state().camera_options(None).zoom);
}

/// A camera at rest reads only the height and floor at its center, which
/// is all the renderer compares to tell whether a frame's camera stood on
/// its newest ground: on ground that differs everywhere but there, it
/// doesn't move, though its eye is far below that ground.
#[test]
fn a_camera_at_rest_reads_only_the_center() {
    let run_loop = Queue::<()>::new();
    for mode in [MapMode::Continuous, MapMode::Static] {
        let mut map = pitchable(STYLE, &run_loop);
        if mode == MapMode::Static {
            map = self::map(mode, &run_loop);
            map.render_still().unwrap();
        }
        map.jump_to(&CameraOptions { zoom: Some(12.0), pitch: Some(60.0), ..Default::default() });
        report(&mut map, ground_event(&flat(Some(500.0), 300.0)));
        assert_eq!((500.0, Some(300.0)), ground(&map), "{mode:?}");
        let view = map.transform_state().view().clone();

        let center = view.center(WrapMode::Wrapped);
        let elsewhere = Arc::new(CenterOnly { center, reads_elsewhere: AtomicU32::new(0) });
        let ground_: Arc<dyn Elevation> = Arc::<CenterOnly>::clone(&elsewhere);
        report(&mut map, ground_event(&ground_));
        map.trigger_repaint();
        assert_eq!(view, *map.transform_state().view(), "{mode:?}");
        assert_eq!(0, elsewhere.reads_elsewhere.load(Ordering::SeqCst), "{mode:?}");
    }
}
