//! The camera over terrain on the globe, which follows the rules it follows
//! on Mercator, with heights along each point's vertical: it stands on the
//! ground at rest, a command ends on it, and the eye stays above it.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3};
use mln_map_types::{Elevation, GroundHit, View};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::queue::Queue;
use mln_util::time::Instant;

use super::ground::{Plateau, STYLE, Slope, assert_near, flat, ground, ground_event, pitchable, report, world};
use super::terrain_camera::{frames, start};
use super::terrain_gestures::meters;
use crate::{
    AnimationOptions, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, Map,
    MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumGlide,
};

/// A 512 by 512 map of [`STYLE`] under `vertical-perspective`, at rest at
/// [`start`] at zoom 14 and `pitch`, with its frame clock an hour ahead so
/// that the test steps it, over `ground`.
fn globe_over(ground: &Arc<dyn Elevation>, pitch: f64, run_loop: &Queue<()>) -> (Map, Instant) {
    let style = STYLE.replace(r#""layers": []"#, r#""layers": [], "projection": {"type": "vertical-perspective"}"#);
    let mut map = pitchable(&style, run_loop);
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    map.jump_to(&CameraOptions { center: Some(start()), zoom: Some(14.0), pitch: Some(pitch), ..Default::default() });
    report(&mut map, ground_event(ground));
    assert!(map.transform_state().view().is_globe());
    (map, clock)
}

fn animated(map: &mut Map, mode: CameraUpdateMode, camera: CameraOptions) {
    map.update_camera(&CameraUpdate {
        mode,
        camera,
        animation: AnimationOptions {
            duration: Some(Duration::from_secs(1)),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
        ..Default::default()
    });
}

/// At rest on the globe the center stands on the ground under it, and the
/// view's planes fit the ground there: the center draws at the middle of
/// the view standing on that ground.
#[test]
fn a_camera_at_rest_on_the_globe_stands_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let (map, _) = globe_over(&flat(Some(1200.0), 1000.0), 60.0, &run_loop);
    assert_eq!((1200.0, Some(1000.0)), ground(&map));
    let center = map.transform_state().view().center(WrapMode::Wrapped);
    let pixel = map.pixel_for_lat_lng(&center);
    assert!((pixel.x - 256.0).hypot(pixel.y - 256.0) < 0.01, "the center draws at {pixel:?}");
}

/// A flight on the globe over ground that rises toward its end ends at the
/// center and zoom it asked for, standing on the ground there.
#[test]
fn a_flight_on_the_globe_ends_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let slope: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise: 500_000.0 });
    let (mut map, clock) = globe_over(&slope, 40.0, &run_loop);
    assert_near(1000.0, ground(&map).0, 1e-6, "the altitude at the start");
    let destination = LatLng::new(20.05, 10.2).unwrap();
    let camera = CameraOptions { center: Some(destination), zoom: Some(13.0), ..Default::default() };
    animated(&mut map, CameraUpdateMode::Fly, camera);
    frames(&mut map, clock, Duration::from_secs(1), |_, _| {});

    assert!(!map.snapshot().transition_in_progress);
    let camera = map.transform_state().camera_options(None);
    let center = camera.center.unwrap();
    assert_near(destination.latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(destination.longitude(), center.longitude(), 1e-9, "longitude");
    assert_eq!(Some(13.0), camera.zoom);
    let end = slope.at(center.wrapped()).unwrap();
    assert!(end > 1200.0, "{end}");
    assert_eq!(end, camera.center_altitude.unwrap());
}

/// A pitch command on the globe that would carry the eye into a plateau
/// behind the center keeps the eye, and the near plane, above it on every
/// frame.
#[test]
fn the_eye_stays_above_a_plateau_while_a_pitch_command_runs_on_the_globe() {
    let run_loop = Queue::<()>::new();
    // The plateau stands under the eye, south of the center, which stands
    // at sea level.
    let plateau = Plateau { y: world(&start()).y + 1e-6, height: 2500.0 };
    let surface = |point: DVec3| plateau.surface_at(point.truncate()).unwrap();
    let ground: Arc<dyn Elevation> = Arc::new(Plateau { ..plateau });
    let (mut map, clock) = globe_over(&ground, 0.0, &run_loop);
    animated(&mut map, CameraUpdateMode::Ease, CameraOptions { pitch: Some(80.0), ..Default::default() });
    let mut clearance = f64::INFINITY;
    frames(&mut map, clock, Duration::from_secs(1), |map, elapsed| {
        let view = map.transform_state().view();
        let eye = view.eye_position();
        assert!(eye.z >= surface(eye) - 0.01, "{elapsed:?}: the eye at {eye}");
        // Each raise turns the near plane down a little, so after GL JS's
        // three the near plane may stay a few millimeters short.
        for point in view.near_plane_points() {
            assert!(point.z >= surface(point) - 0.01, "{elapsed:?}: the near plane at {point}");
        }
        clearance = clearance.min(eye.z - surface(eye));
    });
    let camera = map.transform_state().camera_options(None);
    assert!(camera.pitch.unwrap() < 80.0 && camera.zoom.unwrap() < 14.0, "raised by pitch and zoom: {camera:?}");
    assert!(clearance < 100.0, "the eye came down to {clearance} m above the plateau");
    let center = map.transform_state().view().center(WrapMode::Unwrapped);
    assert_near(start().latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(start().longitude(), center.longitude(), 1e-9, "longitude");
}

/// The plateau the gesture tests grab: 2000 m high from 300 m south of the
/// start on, under the lower part of the view and the eye, which stands
/// about 640 m above it. The center stands at sea level.
fn plateau() -> Arc<dyn Elevation> {
    Arc::new(Plateau { y: world(&start()).y + 300.0 / meters(), height: 2000.0 })
}

/// A screen point (y down) as the view takes it, from the bottom edge.
fn view_point(view: &View, point: DVec2) -> ScreenCoordinate {
    ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
}

/// The ground drawn under the screen point (y down) `point` of `map`.
fn grab(map: &Map, point: DVec2) -> GroundHit {
    let view = map.transform_state().view();
    view.ground_under(&view_point(view, point), &*map.ground().unwrap()).expect("ground under the point")
}

/// How far from the screen point (y down) `point` the ground `hit` draws
/// on `view`, in pixels.
fn off_by(view: &View, hit: &GroundHit, point: DVec2) -> f64 {
    let drawn = view.lat_lng_to_screen_coordinate_at(&hit.location(WrapMode::Unwrapped), hit.height);
    let point = view_point(view, point);
    (drawn.x - point.x).hypot(drawn.y - point.y)
}

/// How far from `to` the sea-level sphere's solve would leave the ground
/// `hit` that the pointer grabbed at `from` (both y down) on `map`: the
/// center that holds the sea-level location under `from` at `to`.
fn off_by_at_sea_level(map: &Map, hit: &GroundHit, from: DVec2, to: DVec2) -> f64 {
    let mut state = map.transform_state().clone();
    let view = state.view();
    let location = view.screen_coordinate_to_lat_lng_at(&view_point(view, from), 0.0, WrapMode::Unwrapped).unwrap();
    let anchor = view_point(view, to);
    state.move_lat_lng_at(&location, &anchor, 0.0).unwrap();
    off_by(state.view(), hit, to)
}

/// A drag's step on the globe that moves the pointer by `offset` to
/// `pointer`.
fn drag_step(map: &mut Map, phase: GesturePhase, offset: DVec2, pointer: DVec2) {
    map.apply_camera_delta(&CameraDelta {
        offset: Some(ScreenCoordinate::new(offset.x, offset.y)),
        anchor: Some(ScreenCoordinate::new(pointer.x, pointer.y)),
        phase,
        ..Default::default()
    })
    .unwrap();
}

/// A drag of 100 pixels on the globe over a plateau 2000 m high keeps the
/// ground it grabbed under the pointer, with the bearing held and with it
/// free, where solving on the sea-level sphere would carry the plateau about
/// four times as far as the pointer.
#[test]
fn a_drag_on_the_globe_holds_the_grabbed_plateau() {
    for free_bearing in [false, true] {
        let run_loop = Queue::<()>::new();
        let (mut map, _) = globe_over(&plateau(), 40.0, &run_loop);
        map.set_globe_free_bearing(free_bearing);
        let (from, to) = (DVec2::new(200.0, 440.0), DVec2::new(300.0, 440.0));
        let grabbed = grab(&map, from);
        assert_eq!(2000.0, grabbed.height, "the pointer grabs the plateau");
        let before = off_by_at_sea_level(&map, &grabbed, from, to);
        assert!(before > 20.0, "the sea-level sphere's solve leaves the plateau {before} px off");

        drag_step(&mut map, GesturePhase::Begin, to - from, to);
        let error = off_by(map.transform_state().view(), &grabbed, to);
        assert!(error < 0.01, "free bearing {free_bearing}: the plateau is {error} px from the pointer");
        assert_eq!(0.0, map.transform_state().view().center_altitude(), "nothing lifts the camera");
    }
}

/// A drag on the globe in ten steps over a plateau 2000 m high keeps the
/// ground it grabbed under the pointer at every step.
#[test]
fn a_held_drag_on_the_globe_holds_the_grabbed_plateau() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = globe_over(&plateau(), 40.0, &run_loop);
    let mut pointer = DVec2::new(200.0, 460.0);
    let grabbed = grab(&map, pointer);
    assert_eq!(2000.0, grabbed.height, "the pointer grabs the plateau");
    let offset = DVec2::new(8.0, -6.0);
    for step in 0..10 {
        pointer += offset;
        let phase = if step == 0 { GesturePhase::Begin } else { GesturePhase::Update };
        drag_step(&mut map, phase, offset, pointer);
        let error = off_by(map.transform_state().view(), &grabbed, pointer);
        assert!(error < 0.01, "step {step}: the plateau is {error} px from the pointer");
    }
}

/// A scroll's zoom around a point on a plateau 2000 m high on the globe, a
/// pinch that pans and zooms around it, and the glide that carries the
/// pinch on hold the ground under the point at every step and frame. They
/// zoom in no farther than keeps the eye well above the plateau, where no
/// lift moves the camera.
#[test]
fn a_zoom_around_a_plateau_on_the_globe_holds_it() {
    let anchor = DVec2::new(300.0, 440.0);
    let around = Some(ScreenCoordinate::new(anchor.x, anchor.y));
    let run_loop = Queue::<()>::new();

    let (mut map, _) = globe_over(&plateau(), 40.0, &run_loop);
    let grabbed = grab(&map, anchor);
    assert_eq!(2000.0, grabbed.height, "the anchor is on the plateau");
    for (step, scale) in [1.1, 0.8, 0.8, 1.25, 0.9].into_iter().enumerate() {
        let phase = if step == 0 { GesturePhase::Begin } else { GesturePhase::Update };
        map.apply_camera_delta(&CameraDelta { scale: Some(scale), anchor: around, phase, ..Default::default() })
            .unwrap();
        let error = off_by(map.transform_state().view(), &grabbed, anchor);
        assert!(error < 0.01, "scroll step {step}: the plateau is {error} px from the anchor");
    }
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "nothing lifts the camera");

    let (mut map, clock) = globe_over(&plateau(), 40.0, &run_loop);
    let offset = DVec2::new(-20.0, 10.0);
    let grabbed = grab(&map, anchor - offset);
    assert_eq!(2000.0, grabbed.height, "the pinch grabs the plateau");
    map.apply_camera_delta(&CameraDelta {
        offset: Some(ScreenCoordinate::new(offset.x, offset.y)),
        scale: Some(0.9),
        anchor: around,
        phase: GesturePhase::Begin,
        ..Default::default()
    })
    .unwrap();
    let error = off_by(map.transform_state().view(), &grabbed, anchor);
    assert!(error < 0.01, "the pinch: the plateau is {error} px from the anchor");

    let decay = MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: 1.0,
            duration: Duration::from_secs(1),
            exponent: 0.5,
            easing: MomentumEasing::ANDROID_FLING,
        },
        ..MomentumDecay::DEFAULT
    };
    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(MomentumGlide { velocity: -1.0, decay });
    momentum.zoom.anchor = around;
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), phase: GesturePhase::End, ..Default::default() })
        .unwrap();
    let zoom = map.transform_state().view().zoom();
    frames(&mut map, clock, Duration::from_millis(992), |map, elapsed| {
        let error = off_by(map.transform_state().view(), &grabbed, anchor);
        assert!(error < 0.01, "{elapsed:?} into the glide: the plateau is {error} px from the anchor");
    });
    let glided = zoom - map.transform_state().view().zoom();
    assert!(glided > 0.1, "the glide zoomed out by {glided}");
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "nothing lifts the camera");
}
