//! How gestures and their momentum move the camera over the terrain: the
//! drawn ground the pointer grabbed stays under it, the gesture holds the
//! altitude, lifted only to keep the eye above the ground, and its end, or
//! its momentum's, comes to rest on the ground with the eye and the picture
//! where they were.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3};
use mln_map_types::{ConstrainMode, Elevation, GroundHit, View, ViewportMode};
use mln_style::constants::DEFAULT_TRANSITION_EASE;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::size::Size;
use mln_util::geo::ScreenCoordinate;
use mln_util::queue::Queue;

use super::ground::{Plateau, STYLE, Slope, assert_near, flat, ground_event, pitchable, report, world};
use super::terrain_camera::{Hill, frames, map_over, meters, start};
use crate::transform::{TerrainGround, Transform, TransformEvent};
use crate::{
    AnchoredMomentum, AnimationOptions, CameraChangeSource, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate,
    CameraUpdateMode, GesturePhase, Map, MapEvent, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumGlide,
};

/// One pixel at zoom 12, in Web Mercator at zoom 0.
const PIXEL: f64 = 1.0 / (512.0 * 4096.0);

/// The middle of the 512 by 512 view.
const MIDDLE: DVec2 = DVec2::new(256.0, 256.0);

/// A ridge 800 m high running east and west, 100 pixels at zoom 12 south of
/// the start and 60 pixels wide on each side, on sea level.
#[derive(Debug)]
struct Ridge {
    y: f64,
}

impl Ridge {
    const PEAK: f64 = 800.0;

    fn drawn() -> Arc<dyn Elevation> {
        Arc::new(Ridge { y: world(&start()).y + 100.0 * PIXEL })
    }

    fn height(&self, world: DVec2) -> f64 {
        Self::PEAK * (1.0 - (world.y - self.y).abs() / (60.0 * PIXEL)).max(0.0)
    }
}

impl Elevation for Ridge {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(self.height(world(&location.wrapped())))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(0.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(self.height(world))
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((0.0, Self::PEAK))
    }
}

/// Ground 500 m high that the renderer drew only south of a line 50 pixels
/// at zoom 12 north of the start: farther north, a pointer grabs nothing.
#[derive(Debug)]
struct Patch {
    y: f64,
}

impl Patch {
    const HEIGHT: f64 = 500.0;

    fn drawn() -> Arc<dyn Elevation> {
        Arc::new(Patch { y: world(&start()).y - 50.0 * PIXEL })
    }

    fn height(&self, world: DVec2) -> Option<f64> {
        (world.y >= self.y).then_some(Self::HEIGHT)
    }
}

impl Elevation for Patch {
    fn at(&self, location: LatLng) -> Option<f64> {
        self.height(world(&location.wrapped()))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(Self::HEIGHT)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        self.height(world)
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((Self::HEIGHT, Self::HEIGHT))
    }
}

/// Ground at sea level under the start that rises about half a meter for
/// each meter east, so that a gesture that holds the start's altitude ends
/// above or below the ground in front of it.
fn slope() -> Arc<dyn Elevation> {
    Arc::new(Slope { x: world(&start()).x, height: 0.0, rise: 2e7 })
}

/// A screen point (y down) as the view takes it, from the bottom edge.
fn view_point(view: &View, point: DVec2) -> ScreenCoordinate {
    ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
}

/// The ground drawn under the screen point (y down) `point` of `view`.
fn grab(view: &View, ground: &dyn Elevation, point: DVec2) -> GroundHit {
    view.ground_under(&view_point(view, point), ground).expect("ground under the point")
}

/// Where the ground `hit` draws on `view`, as a screen point (y down).
fn drawn(view: &View, hit: &GroundHit) -> DVec2 {
    let point = view.lat_lng_to_screen_coordinate_at(&hit.location(WrapMode::Unwrapped), hit.height);
    DVec2::new(point.x, f64::from(view.size().height) - point.y)
}

/// The ground `height` meters up under the screen point (y down) `point`
/// of `view`, on the plane at that height.
fn on_plane(view: &View, point: DVec2, height: f64) -> GroundHit {
    let location = view.screen_coordinate_to_lat_lng_at(&view_point(view, point), height, WrapMode::Unwrapped);
    GroundHit { world: world(&location.unwrap()), height }
}

/// A gesture's step that moves the pointer by `offset` to `anchor`, if it
/// names one.
fn drag(phase: GesturePhase, offset: DVec2, anchor: Option<DVec2>) -> CameraDelta {
    CameraDelta {
        offset: Some(ScreenCoordinate::new(offset.x, offset.y)),
        anchor: anchor.map(|anchor| ScreenCoordinate::new(anchor.x, anchor.y)),
        phase,
        ..Default::default()
    }
}

fn phase(step: u32) -> GesturePhase {
    if step == 0 { GesturePhase::Begin } else { GesturePhase::Update }
}

/// A glide that lasts `seconds` from `speed`, along a curve that ends at
/// rest.
pub(super) fn decay(seconds: u64, speed: f64) -> MomentumDecay {
    MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: speed,
            duration: Duration::from_secs(seconds),
            exponent: 0.5,
            easing: MomentumEasing::ANDROID_FLING,
        },
        ..MomentumDecay::DEFAULT
    }
}

/// A drag that carries a ridge across the screen keeps the ground it
/// grabbed on the ridge's flank under the pointer at every step, where
/// solving at sea level lets it slip by the ridge's parallax. The camera
/// stands far enough above the ridge that no lift moves it.
#[test]
fn a_drag_across_a_ridge_keeps_the_grabbed_ground_under_the_pointer() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn();
    let (mut map, _) = map_over(Some(&ridge), &run_loop);
    let mut pointer = DVec2::new(200.0, 340.0);
    let grabbed = grab(map.transform_state().view(), &*ridge, pointer);
    assert!((100.0..700.0).contains(&grabbed.height), "the pointer grabs the flank at {} m", grabbed.height);
    let offset = DVec2::new(2.0, -6.0);
    let mut highest = 0.0f64;
    for step in 0..40 {
        pointer += offset;
        map.apply_camera_delta(&drag(phase(step), offset, Some(pointer))).unwrap();
        let view = map.transform_state().view();
        let error = drawn(view, &grabbed).distance(pointer);
        assert!(error < 0.01, "step {step}: the grabbed ground is {error} px from the pointer");
        assert_eq!(0.0, view.center_altitude(), "nothing lifts the camera");
        highest = highest.max(grab(view, &*ridge, MIDDLE).height);
    }
    assert!(highest > 700.0, "the ridge passes the middle of the view");
}

/// The first step of a drag over ground 1000 m high moves the ground under
/// the pointer, or under the view's middle without an anchor, by the
/// pointer's move, rather than jumping by the altitude times the tangent of
/// the pitch, about 23 pixels here, as a drag solved at sea level does.
#[test]
fn a_drag_over_raised_ground_moves_it_with_the_pointer() {
    for anchor in [None, Some(DVec2::new(300.0, 350.0))] {
        let run_loop = Queue::<()>::new();
        let (mut map, _) = map_over(Some(&flat(Some(1000.0), 1000.0)), &run_loop);
        assert_eq!(1000.0, map.transform_state().view().center_altitude());
        let offset = DVec2::new(0.0, 1.0);
        let pointer = anchor.unwrap_or(MIDDLE);
        let grabbed = on_plane(map.transform_state().view(), pointer - offset, 1000.0);
        map.apply_camera_delta(&drag(GesturePhase::Begin, offset, anchor)).unwrap();
        let error = drawn(map.transform_state().view(), &grabbed).distance(pointer);
        assert!(error < 0.01, "anchor {anchor:?}: the ground is {error} px from the pointer");
    }
}

/// A pinch's zoom, a turn and a tilt around a point up a slope hold the
/// ground under it, and so does the zoom's glide on every frame.
#[test]
fn a_pinch_turn_tilt_and_glide_around_a_point_on_a_slope_hold_it() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    let anchor = DVec2::new(400.0, 330.0);
    let grabbed = grab(map.transform_state().view(), &*ground, anchor);
    assert!(grabbed.height > 1000.0, "the anchor is up the slope, at {} m", grabbed.height);
    let around = Some(ScreenCoordinate::new(anchor.x, anchor.y));
    let steps = [
        CameraDelta { scale: Some(1.4), anchor: around, phase: GesturePhase::Begin, ..Default::default() },
        CameraDelta { bearing: Some(25.0), anchor: around, phase: GesturePhase::Update, ..Default::default() },
        CameraDelta {
            scale: Some(0.8),
            bearing: Some(-10.0),
            anchor: around,
            phase: GesturePhase::Update,
            ..Default::default()
        },
        CameraDelta { pitch: Some(8.0), anchor: around, phase: GesturePhase::Update, ..Default::default() },
    ];
    for (step, delta) in steps.iter().enumerate() {
        map.apply_camera_delta(delta).unwrap();
        let error = drawn(map.transform_state().view(), &grabbed).distance(anchor);
        assert!(error < 0.01, "step {step}: the ground moved {error} px from the anchor");
    }

    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(MomentumGlide { velocity: 1.0, decay: decay(1, 1.0) });
    momentum.zoom.anchor = around;
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), phase: GesturePhase::End, ..Default::default() })
        .unwrap();
    let zoom = map.transform_state().view().zoom();
    frames(&mut map, clock, Duration::from_millis(992), |map, elapsed| {
        let error = drawn(map.transform_state().view(), &grabbed).distance(anchor);
        assert!(error < 0.01, "{elapsed:?} into the glide: the ground is {error} px from the anchor");
    });
    let glided = map.transform_state().view().zoom() - zoom;
    assert!(glided > 0.1, "the glide zoomed in by {glided}");
}

/// A pointer over ground the renderer didn't draw grabs nothing: the drag
/// then moves the map on the plane at the center's altitude, as a drag
/// without an anchor does.
#[test]
fn a_grab_in_the_sky_pans_on_the_center_plane() {
    let run_loop = Queue::<()>::new();
    let patch = Patch::drawn();
    let pan = |anchor: Option<DVec2>| {
        let (mut map, _) = map_over(Some(&patch), &run_loop);
        assert_eq!(Patch::HEIGHT, map.transform_state().view().center_altitude());
        let offset = DVec2::new(10.0, 20.0);
        if let Some(anchor) = anchor {
            let view = map.transform_state().view();
            assert!(view.ground_under(&view_point(view, anchor - offset), &*patch).is_none(), "grabs nothing");
        }
        for step in 0..3 {
            let moving = on_plane(map.transform_state().view(), MIDDLE - offset, Patch::HEIGHT);
            map.apply_camera_delta(&drag(phase(step), offset, anchor)).unwrap();
            let error = drawn(map.transform_state().view(), &moving).distance(MIDDLE);
            assert!(error < 0.01, "anchor {anchor:?}, step {step}: the center's plane is {error} px off");
        }
        map.transform_state().camera_options(None)
    };
    assert_eq!(pan(None), pan(Some(DVec2::new(266.0, 40.0))));
}

/// A drag on the globe in ten steps over a plateau 2000 m high below the
/// center, with the bearing held and free, keeps the ground it grabbed
/// under the pointer at every step, where a drag solved on the sea-level
/// sphere carries the plateau farther than the pointer.
#[test]
fn a_drag_on_the_globe_holds_the_grabbed_plateau() {
    for free_bearing in [false, true] {
        let run_loop = Queue::<()>::new();
        let style = STYLE.replace(r#""layers": []"#, r#""layers": [], "projection": {"type": "vertical-perspective"}"#);
        let mut map = pitchable(&style, &run_loop);
        map.jump_to(&CameraOptions {
            center: Some(start()),
            zoom: Some(14.0),
            pitch: Some(40.0),
            ..Default::default()
        });
        let plateau: Arc<dyn Elevation> = Arc::new(Plateau { y: world(&start()).y + 300.0 / meters(), height: 2000.0 });
        report(&mut map, ground_event(&plateau));
        map.set_globe_free_bearing(free_bearing);
        assert!(map.transform_state().view().is_globe());

        let mut pointer = DVec2::new(200.0, 460.0);
        let grabbed = grab(map.transform_state().view(), &*plateau, pointer);
        assert_eq!(2000.0, grabbed.height, "the pointer grabs the plateau");
        let offset = DVec2::new(8.0, -6.0);
        for step in 0..10 {
            pointer += offset;
            map.apply_camera_delta(&drag(phase(step), offset, Some(pointer))).unwrap();
            let error = drawn(map.transform_state().view(), &grabbed).distance(pointer);
            assert!(error < 0.01, "free bearing {free_bearing}, step {step}: the plateau is {error} px off");
        }
        assert_eq!(0.0, map.transform_state().view().center_altitude(), "nothing lifts the camera");
    }
}

/// How far the eye, or a point of the near plane, of `view` is below
/// `ground`, in meters: zero or less while they are clear.
fn under_ground(view: &View, ground: &dyn Elevation) -> f64 {
    std::iter::once(view.eye_position())
        .chain(view.near_plane_points())
        .map(|point| ground.surface_at(point.truncate()).unwrap() - point.z)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// A drag at zoom 15 and pitch 60, the pointer low on the screen, that
/// carries the camera north over a hill 3000 m high, which rises above the
/// eye: the gesture keeps the pitch and the zoom its fingers set, and the
/// eye and the near plane above the ground at every step, lifted no faster
/// than the 45-degree hill rises under them, and lowers to the altitude the
/// gesture holds once the hill is past. Each step that doesn't lift holds
/// the ground the pointer grabbed under it, unless that ground stands in
/// the top tenth of the way up from the center to the eye, so near that
/// holding it would stall the drag: then the map moves on all the same.
#[test]
fn a_drag_over_a_hill_lifts_the_eye_over_it_and_lowers_past_it() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(None, &run_loop);
    map.jump_to(&CameraOptions { zoom: Some(15.0), pitch: Some(60.0), ..Default::default() });
    let hill = Hill::new(8000.0);
    let drawn_hill: Arc<dyn Elevation> = Arc::new(Hill::new(8000.0));
    report(&mut map, ground_event(&drawn_hill));
    assert!(map.transform_state().view().eye_position().z < 3000.0, "the hill rises above the eye");

    let pointer = DVec2::new(256.0, 450.0);
    let offset = DVec2::new(0.0, 12.0);
    let (mut highest, mut slowest, mut steps) = (0.0f64, f64::INFINITY, 0);
    let mut passed = false;
    while !passed {
        let before = map.transform_state().view().clone();
        let grabbed = grab(&before, &*drawn_hill, pointer - offset);
        map.apply_camera_delta(&drag(phase(steps), offset, Some(pointer))).unwrap();
        let view = map.transform_state().view();
        assert_near(60.0, view.pitch().to_degrees(), 1e-9, "the pitch");
        assert_near(15.0, view.zoom(), 1e-9, "the zoom");
        let under = under_ground(view, &hill);
        assert!(under <= 1e-6, "step {steps}: the eye or the near plane is {under} m under the ground");
        let (altitude, eye) = (view.center_altitude(), view.eye_position());
        let rose = (altitude - before.center_altitude()).abs();
        let eye_moved = (eye.truncate() - before.eye_position().truncate()).length() * meters();
        assert!(rose <= eye_moved + 0.01, "step {steps}: lifted {rose} m as the eye moved {eye_moved} m");
        let held = before.center_altitude();
        let near_eye = grabbed.height - held >= 0.9 * (before.eye_position().z - held);
        let slipped = drawn(view, &grabbed).distance(pointer);
        if rose == 0.0 && !near_eye {
            assert!(slipped < 0.01, "step {steps}: the grabbed ground is {slipped} px from the pointer");
        }
        let moved = (world(&view.center(WrapMode::Unwrapped)) - world(&before.center(WrapMode::Unwrapped))).length();
        slowest = slowest.min(moved * meters());
        highest = highest.max(altitude);
        passed = world(&view.center(WrapMode::Unwrapped)).y < world(&start()).y - 16000.0 / meters();
        steps += 1;
        assert!(steps < 5000, "the drag didn't pass the hill");
    }
    assert!(highest > 500.0, "the hill lifted the camera {highest} m");
    assert!(slowest > 2.0, "a step moved the map only {slowest} m");
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "the lift lowers past the hill");
    assert!(map.snapshot().gesture_in_progress);
}

/// Ground that loads under a gesture held still, above its eye, lifts the
/// camera just over it in the update that brings it, with the center
/// clamped or not.
#[test]
fn ground_that_loads_under_a_held_gesture_lifts_the_eye_over_it() {
    for clamped in [true, false] {
        let run_loop = Queue::<()>::new();
        let (mut map, _) = map_over(Some(&flat(Some(0.0), 0.0)), &run_loop);
        map.set_center_clamped_to_ground(clamped);
        map.jump_to(&CameraOptions { zoom: Some(16.0), pitch: Some(60.0), ..Default::default() });
        map.apply_camera_delta(&drag(GesturePhase::Begin, DVec2::new(0.0, 1.0), None)).unwrap();
        let held = map.transform_state().camera_options(None);
        let high = flat(Some(3000.0), 3000.0);
        report(&mut map, ground_event(&high));
        let view = map.transform_state().view();
        let under = under_ground(view, &*high);
        assert!((-1e-6..=1e-6).contains(&under), "clamped {clamped}: {under} m under the ground");
        let lifted = map.transform_state().camera_options(None);
        assert_eq!((held.center, held.zoom, held.pitch), (lifted.center, lifted.zoom, lifted.pitch));
    }
}

/// The ground under three points of the screen: the center, and points left
/// and right of it.
fn picture_points(view: &View, ground: &dyn Elevation) -> Vec<GroundHit> {
    [(256.0, 256.0), (100.0, 200.0), (412.0, 200.0)]
        .into_iter()
        .map(|(x, y)| grab(view, ground, DVec2::new(x, y)))
        .collect()
}

/// Checks that `map`'s eye stands within a meter of `eye`, a few
/// hundredths of a pixel at zoom 12, that the ground at `points` draws
/// within 0.05 px of where it did, at `pixels`,
/// that the center stands on the ground, and that the camera is at rest:
/// the next update moves it no more, and reports no change.
fn assert_came_to_rest(map: &mut Map, ground: &dyn Elevation, eye: DVec3, points: &[GroundHit], pixels: &[DVec2]) {
    let view = map.transform_state().view().clone();
    let moved = view.eye_position() - eye;
    let moved = DVec3::new(moved.x * meters(), moved.y * meters(), moved.z).length();
    assert!(moved < 1.0, "the eye moved {moved} m");
    for (point, before) in points.iter().zip(pixels) {
        let now = drawn(&view, point);
        assert!(now.distance(*before) < 0.05, "the ground at {before} moved to {now}");
    }
    let center = view.center(WrapMode::Wrapped);
    assert_eq!(ground.at(center).unwrap(), view.center_altitude(), "the center stands on the ground");
    map.trigger_repaint();
    assert_eq!(view, *map.transform_state().view(), "the next update moves the camera");
    assert_eq!(Vec::<(bool, CameraChangeSource)>::new(), camera_events(map));
}

/// The camera events since the last drain, as (whether it is a
/// `CameraDidChange`, its source).
fn camera_events(map: &mut Map) -> Vec<(bool, CameraChangeSource)> {
    map.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            MapEvent::CameraWillChange(change) => Some((false, change.source)),
            MapEvent::CameraDidChange(change) => Some((true, change.source)),
            _ => None,
        })
        .collect()
}

/// A gesture's step that pans by `offset` or zooms by `scale` around a
/// point.
fn step(phase: GesturePhase, offset: Option<(f64, f64)>, scale: Option<f64>) -> CameraDelta {
    CameraDelta {
        offset: offset.map(|(x, y)| ScreenCoordinate::new(x, y)),
        scale,
        anchor: scale.map(|_| ScreenCoordinate::new(100.0, 150.0)),
        phase,
        ..Default::default()
    }
}

/// A gesture that held the altitude while it panned and zoomed up a slope
/// ends with its center moved onto the slope along the view: neither the
/// eye nor the ground on the screen moves, and the end reports the change
/// before the camera is at rest.
#[test]
fn a_gesture_end_comes_to_rest_without_moving_the_picture() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, _) = map_over(Some(&ground), &run_loop);
    map.apply_camera_delta(&step(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    map.apply_camera_delta(&step(GesturePhase::Update, None, Some(1.5))).unwrap();
    map.apply_camera_delta(&step(GesturePhase::Update, Some((-40.0, 10.0)), None)).unwrap();
    let held = map.transform_state().camera_options(None);
    assert_eq!(Some(0.0), held.center_altitude, "the gesture holds the altitude");
    let view = map.transform_state().view();
    let eye = view.eye_position();
    let points = picture_points(view, &*ground);
    let pixels: Vec<_> = points.iter().map(|point| drawn(view, point)).collect();
    camera_events(&mut map);

    map.apply_camera_delta(&step(GesturePhase::End, None, None)).unwrap();
    let ended = map.transform_state().camera_options(None);
    let altitude = ended.center_altitude.unwrap();
    assert!(altitude > 200.0, "the center moved onto the slope, to {altitude} m");
    assert!(ended.zoom.unwrap() > held.zoom.unwrap() + 0.01, "the zoom follows the nearer center");
    assert_eq!((held.pitch, held.bearing), (ended.pitch, ended.bearing));
    let gesture = CameraChangeSource::Gesture;
    assert_eq!(vec![(false, gesture), (true, gesture)], camera_events(&mut map), "the end");
    assert_came_to_rest(&mut map, &*ground, eye, &points, &pixels);
}

/// The end of a gesture's glide, too, moves the center onto the ground
/// along the view, with the eye and the picture where its last frame left
/// them, before the glide's `CameraDidChange`.
#[test]
fn a_glide_end_comes_to_rest_without_moving_the_picture() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    map.apply_camera_delta(&step(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    let glide = MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay: decay(1, 1000.0) };
    let momentum = CameraMomentum { pan: Some(glide), ..Default::default() };
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..step(GesturePhase::End, None, None) }).unwrap();
    assert!(map.snapshot().gesture_in_progress);

    // A tenth of a millisecond before the end, the glide has all but stopped.
    map.set_frame_time(Some(clock + Duration::from_micros(999_900)));
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "the glide holds the altitude");
    let view = map.transform_state().view();
    let eye = view.eye_position();
    let points = picture_points(view, &*ground);
    let pixels: Vec<_> = points.iter().map(|point| drawn(view, point)).collect();
    camera_events(&mut map);
    map.set_frame_time(Some(clock + Duration::from_secs(1)));
    let snapshot = map.snapshot();
    assert!(!snapshot.transition_in_progress && !snapshot.gesture_in_progress);
    let altitude = map.transform_state().view().center_altitude();
    assert!(altitude.abs() > 500.0, "the center moved onto the slope, to {altitude} m");
    assert_eq!(vec![(true, CameraChangeSource::Momentum)], camera_events(&mut map), "the glide's end");
    assert_came_to_rest(&mut map, &*ground, eye, &points, &pixels);
}

/// A momentum with nothing to move ends its gesture at once, and the end's
/// move onto the ground reports its change as every camera change does:
/// will change, the command's end, did change.
#[test]
fn an_empty_momentum_reports_its_rest_in_order() {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(slope()), clamped: true }));
    transform.jump_to(&CameraOptions {
        center: Some(start()),
        zoom: Some(12.0),
        pitch: Some(40.0),
        ..Default::default()
    });
    transform.follow_terrain();
    transform.apply_camera_delta(&step(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    let held = transform.view().center_altitude();
    transform.drain_events().for_each(drop);
    let momentum = CameraMomentum::default();
    transform
        .apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..step(GesturePhase::End, None, None) })
        .unwrap();
    assert!(!transform.is_gesture_in_progress());
    assert_ne!(held, transform.view().center_altitude(), "the end came to rest on the ground");
    let events: Vec<_> = transform
        .drain_events()
        .map(|event| match event {
            TransformEvent::CameraWillChange(change) => format!("will {:?}", change.source),
            TransformEvent::CameraIsChanging => "changing".to_owned(),
            TransformEvent::CameraDidChange(change) => format!("did {:?}", change.source),
            TransformEvent::CommandEnded(_) => "ended".to_owned(),
        })
        .collect();
    assert_eq!(vec!["will Momentum", "ended", "did Momentum"], events);
}

/// A command that takes the center from a gesture's glide, once the fingers
/// are up, eases its altitude onto the ground it ends on, as any command
/// does: its last frame stands on that ground, and neither the glide's end,
/// the turn the glide goes on with, nor the camera at rest moves the center,
/// the zoom or the altitude after it.
#[test]
fn a_command_that_takes_the_center_from_a_glide_ends_on_the_ground() {
    let destination = LatLng::new(20.0, 10.02).unwrap();
    for turns in [false, true] {
        let run_loop = Queue::<()>::new();
        let ground = slope();
        let (mut map, clock) = map_over(Some(&ground), &run_loop);
        map.apply_camera_delta(&step(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
        let momentum = CameraMomentum {
            pan: Some(MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay: decay(1, 1000.0) }),
            bearing: AnchoredMomentum {
                glide: turns.then_some(MomentumGlide { velocity: 30.0, decay: decay(3, 30.0) }),
                ..Default::default()
            },
            ..Default::default()
        };
        map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..step(GesturePhase::End, None, None) })
            .unwrap();
        map.set_frame_time(Some(clock + Duration::from_millis(96)));
        let from = map.transform_state().view().center_altitude();
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions { center: Some(destination), ..Default::default() },
            animation: AnimationOptions {
                duration: Some(Duration::from_secs(1)),
                easing: Some(DEFAULT_TRANSITION_EASE),
                ..Default::default()
            },
            ..Default::default()
        });
        let end = ground.at(destination).unwrap();
        assert!(end > from + 500.0, "{from} m to {end} m");
        // Every frame stands on the line from where it started to that
        // ground, at the ease's progress.
        frames(&mut map, clock + Duration::from_millis(96), Duration::from_secs(1), |map, elapsed| {
            let k = DEFAULT_TRANSITION_EASE.solve(elapsed.as_secs_f64(), 0.001);
            let altitude = map.transform_state().view().center_altitude();
            assert_near(from + (end - from) * k, altitude, 1e-6, &format!("turns {turns}: at {elapsed:?}"));
        });
        let ended = map.transform_state().camera_options(None);
        assert_eq!(Some(end), ended.center_altitude, "turns {turns}: the command ends on the ground");
        assert_eq!(turns, map.snapshot().gesture_in_progress, "turns {turns}");
        map.set_frame_time(Some(clock + Duration::from_secs(5)));
        map.trigger_repaint();
        assert!(!map.snapshot().gesture_in_progress);
        let rest = map.transform_state().camera_options(None);
        assert_eq!(
            (ended.center, ended.zoom, ended.center_altitude),
            (rest.center, rest.zoom, rest.center_altitude),
            "turns {turns}: at rest"
        );
    }
}
