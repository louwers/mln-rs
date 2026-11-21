//! How a gesture's steps move the camera over the terrain: the ground the
//! pointer grabbed stays under it, on the plane at the grabbed ground's
//! height, and steps without a usable anchor move the map on the plane at
//! the center's altitude.

use std::sync::Arc;
use std::time::Duration;

use glam::DVec2;
use mln_map_types::{Elevation, GroundHit, View};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::queue::Queue;

use super::ground::{flat, ground_event, report, world};
use super::terrain_camera::{frames, map_over, pixel, start};
use super::terrain_gestures::{Hill, meters};
use crate::{
    AnimationOptions, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode, GesturePhase, Map,
    MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumGlide,
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

/// Ground that rises about 130 m for each kilometer east of the start,
/// where it is at sea level.
fn slope() -> Arc<dyn Elevation> {
    Arc::new(super::ground::Slope { x: world(&start()).x, height: 0.0, rise: 5e6 })
}

/// A view point, measured from the bottom edge, of the screen point (y
/// down) `point`.
fn view_point(view: &View, point: DVec2) -> ScreenCoordinate {
    ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
}

/// Where the point `height` meters up at the Web Mercator point `world`
/// draws on `map`, as a screen point (y down).
fn on_screen(map: &Map, world: DVec2, height: f64) -> DVec2 {
    let view = map.transform_state().view();
    let point = pixel(view, world, height);
    DVec2::new(point.x, f64::from(view.size().height) - point.y)
}

/// The ground drawn under the screen point (y down) `point` of `map`.
fn ground_under(map: &Map, point: DVec2) -> GroundHit {
    let view = map.transform_state().view();
    view.ground_under(&view_point(view, point), &*map.ground().unwrap()).expect("ground under the point")
}

/// The Web Mercator point where the line of sight through the screen point
/// (y down) `point` of `map` meets the plane `height` meters up.
fn on_plane(map: &Map, point: DVec2, height: f64) -> DVec2 {
    let view = map.transform_state().view();
    world(&view.screen_coordinate_to_lat_lng_at(&view_point(view, point), height, WrapMode::Unwrapped).unwrap())
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

/// A drag that carries a ridge across the screen keeps the ground it
/// grabbed on the ridge's flank exactly under the pointer at every step,
/// where solving at sea level would let it slip by the ridge's parallax.
/// The camera stands far enough above the ridge that no lift moves it.
#[test]
fn a_pan_across_a_ridge_keeps_the_grabbed_ground_under_the_pointer() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(Some(&Ridge::drawn()), &run_loop);
    let mut pointer = DVec2::new(200.0, 340.0);
    let grabbed = ground_under(&map, pointer);
    assert!((100.0..700.0).contains(&grabbed.height), "the pointer grabs the flank at {} m", grabbed.height);
    let offset = DVec2::new(2.0, -6.0);
    let mut heights_passed = Vec::new();
    for step in 0..40 {
        pointer += offset;
        map.apply_camera_delta(&drag(phase(step), offset, Some(pointer))).unwrap();
        let error = on_screen(&map, grabbed.world, grabbed.height).distance(pointer);
        assert!(error < 0.01, "step {step}: the grabbed ground is {error} px from the pointer");
        assert_eq!(0.0, map.transform_state().view().center_altitude(), "nothing lifts the camera");
        heights_passed.push(ground_under(&map, MIDDLE).height);
    }
    assert!(heights_passed.iter().any(|&height| height > 700.0), "the ridge passes the middle of the view");
}

/// A pinch's zoom and a turn around a point on a slope hold the ground
/// under it, on the plane at its height.
#[test]
fn a_zoom_and_a_turn_around_a_point_on_a_slope_hold_it() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(Some(&slope()), &run_loop);
    let anchor = DVec2::new(400.0, 330.0);
    let grabbed = ground_under(&map, anchor);
    assert!(grabbed.height > 200.0, "the anchor is up the slope, at {} m", grabbed.height);
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
        let error = on_screen(&map, grabbed.world, grabbed.height).distance(anchor);
        assert!(error < 0.01, "step {step}: the ground moved {error} px from the anchor");
    }
}

/// The first step of a pan over ground 1000 m high moves the ground under
/// the pointer, or under the view's middle without an anchor, by the
/// pointer's move, rather than jumping by the altitude times the tangent of
/// the pitch (about 23 pixels here) as a pan solved at sea level would.
#[test]
fn the_first_pan_over_raised_ground_does_not_jump() {
    for anchor in [None, Some(DVec2::new(300.0, 350.0))] {
        let run_loop = Queue::<()>::new();
        let (mut map, _) = map_over(Some(&flat(Some(1000.0), 1000.0)), &run_loop);
        assert_eq!(1000.0, map.transform_state().view().center_altitude());
        let offset = DVec2::new(0.0, 1.0);
        let pointer = anchor.unwrap_or(MIDDLE);
        let grabbed = on_plane(&map, pointer - offset, 1000.0);
        map.apply_camera_delta(&drag(GesturePhase::Begin, offset, anchor)).unwrap();
        let error = on_screen(&map, grabbed, 1000.0).distance(pointer);
        assert!(error < 0.01, "anchor {anchor:?}: the ground is {error} px from the pointer");
    }
}

/// A pointer over ground the renderer didn't draw grabs nothing: the drag
/// then moves the map on the plane at the center's altitude, as a drag
/// without an anchor does.
#[test]
fn a_grab_in_the_sky_pans_on_the_centre_plane() {
    let run_loop = Queue::<()>::new();
    let pan = |anchor: Option<DVec2>| {
        let (mut map, _) = map_over(Some(&Patch::drawn()), &run_loop);
        assert_eq!(Patch::HEIGHT, map.transform_state().view().center_altitude());
        let offset = DVec2::new(10.0, 20.0);
        if let Some(anchor) = anchor {
            let view = map.transform_state().view();
            let grabbed = view.ground_under(&view_point(view, anchor - offset), &*map.ground().unwrap());
            assert!(grabbed.is_none(), "the pointer grabs no ground");
        }
        for step in 0..3 {
            let moving = on_plane(&map, MIDDLE - offset, Patch::HEIGHT);
            map.apply_camera_delta(&drag(phase(step), offset, anchor)).unwrap();
            let error = on_screen(&map, moving, Patch::HEIGHT).distance(MIDDLE);
            assert!(error < 0.01, "anchor {anchor:?}, step {step}: the center's plane is {error} px off");
        }
        map.transform_state().camera_options(None)
    };
    assert_eq!(pan(None), pan(Some(DVec2::new(266.0, 40.0))));
}

/// The glide that carries on a drag over ground 1000 m high moves the map
/// no faster than its release, from the drag's last step through its first
/// frame to its end, where a glide solved at sea level would jump about 23
/// pixels at its first frame.
#[test]
fn a_glide_carries_on_without_a_jump() {
    let run_loop = Queue::<()>::new();
    let (mut map, clock) = map_over(Some(&flat(Some(1000.0), 1000.0)), &run_loop);
    let offset = DVec2::new(0.0, 8.0);
    let mut pointer = DVec2::new(256.0, 300.0);
    for step in 0..2 {
        pointer += offset;
        map.apply_camera_delta(&drag(phase(step), offset, Some(pointer))).unwrap();
    }
    let speed = 300.0;
    let decay = MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: 1000.0,
            duration: Duration::from_secs(1),
            exponent: 0.5,
            easing: MomentumEasing::ANDROID_FLING,
        },
        ..MomentumDecay::DEFAULT
    };
    let glide = MomentumGlide { velocity: ScreenCoordinate::new(0.0, speed), decay };
    let momentum = CameraMomentum { pan: Some(glide), ..Default::default() };
    let mut middle = on_plane(&map, MIDDLE, 1000.0);
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), phase: GesturePhase::End, ..Default::default() })
        .unwrap();
    let mut glided = 0.0;
    frames(&mut map, clock, Duration::from_secs(2), |map, elapsed| {
        let moved = on_screen(map, middle, 1000.0) - MIDDLE;
        assert!(moved.x.abs() < 1e-6, "{elapsed:?}: the glide turned aside by {} px", moved.x);
        assert!(
            (0.0..=speed * 0.016 * 1.05).contains(&moved.y),
            "{elapsed:?}: the ground moved {} px in a frame",
            moved.y
        );
        glided += moved.y;
        middle = on_plane(map, MIDDLE, 1000.0);
    });
    assert!(glided > 50.0, "the glide moved the map {glided} px");
    assert!(!map.snapshot().gesture_in_progress);
}

/// A drag toward a hill holds the ground it grabbed at sea level on the
/// plane at sea level while the hill lifts the camera: each step moves the
/// map on about as far as the one before, where a pan at the center solved
/// at sea level would jump ahead by the lift times the tangent of the pitch.
/// Each step puts the grabbed ground under the pointer, and the lift that
/// follows moves it on the screen by no more than the lift's change in that
/// step makes it: the next step grabs the ground under the pointer then.
#[test]
fn a_lifted_pan_moves_on_without_a_jump() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(None, &run_loop);
    map.jump_to(&CameraOptions { zoom: Some(14.0), pitch: Some(60.0), ..Default::default() });
    let hill: Arc<dyn Elevation> = Arc::new(Hill::new(8000.0));
    report(&mut map, ground_event(&hill));
    let anchor = DVec2::new(256.0, 400.0);
    let offset = DVec2::new(0.0, 10.0);
    let (mut last, mut highest) = (None::<f64>, 0.0f64);
    for step in 0..250 {
        let view = map.transform_state().view();
        let (center, altitude) = (world(&view.center(WrapMode::Unwrapped)), view.center_altitude());
        let grabbed = on_plane(&map, anchor - offset, 0.0);
        map.apply_camera_delta(&drag(phase(step), offset, Some(anchor))).unwrap();
        let view = map.transform_state().view();
        let lifted = (view.center_altitude() - altitude).abs();
        let error = on_screen(&map, grabbed, 0.0).distance(anchor);
        assert!(error <= 0.01 + 0.25 * lifted, "step {step}: the ground is {error} px off after a {lifted} m lift");
        let moved = (world(&view.center(WrapMode::Unwrapped)) - center).length() * meters();
        if let Some(last) = last {
            assert!((0.95..1.05).contains(&(moved / last)), "step {step}: the map moved {moved} m after {last} m");
        }
        highest = highest.max(view.center_altitude());
        last = Some(moved);
    }
    assert!(highest > 1000.0, "the hill lifted the camera {highest} m");
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "the lift lowers past the hill");
}

/// Where the sky starts on `map`'s screen (y down): above it, a line of
/// sight doesn't descend to the ground.
fn horizon(map: &Map) -> f64 {
    let view = map.transform_state().view();
    view.padding().center_in(view.size()).y - view.mercator_horizon()
}

/// A drag whose pointer comes back below the horizon after a step above it
/// grabs no ground on its way back: it moves the map on the plane at the
/// center's altitude, as a drag without an anchor does, rather than solving
/// for a point in the sky.
#[test]
fn a_drag_back_from_the_sky_pans_on_the_centre_plane() {
    let run_loop = Queue::<()>::new();
    let pan = |anchored: bool| {
        let (mut map, _) = map_over(Some(&slope()), &run_loop);
        map.jump_to(&CameraOptions { pitch: Some(75.0), ..Default::default() });
        let below = DVec2::new(330.0, horizon(&map) + 15.0);
        let offset = DVec2::new(0.0, -30.0);
        // Up across the horizon, then back.
        for (step, (offset, pointer)) in [(offset, below + offset), (-offset, below)].into_iter().enumerate() {
            map.apply_camera_delta(&drag(phase(step as u32), offset, anchored.then_some(pointer))).unwrap();
        }
        map.transform_state().camera_options(None)
    };
    assert_eq!(pan(false), pan(true));
}

/// A drag whose pointer passes through the view's middle keeps the ground
/// it grabbed under the pointer there too.
#[test]
fn a_drag_through_the_middle_keeps_the_grabbed_ground_under_the_pointer() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(Some(&slope()), &run_loop);
    let offset = DVec2::new(-4.0, -4.0);
    let mut pointer = MIDDLE - offset * 11.0;
    let grabbed = ground_under(&map, pointer);
    assert!(grabbed.height > 50.0, "the pointer grabs the slope at {} m", grabbed.height);
    for step in 0..20 {
        pointer += offset;
        map.apply_camera_delta(&drag(phase(step), offset, Some(pointer))).unwrap();
        let error = on_screen(&map, grabbed.world, grabbed.height).distance(pointer);
        assert!(error < 0.01, "step {step} at {pointer}: the grabbed ground is {error} px from the pointer");
    }
}

/// A tilt around a point just below the horizon, which carries it above
/// the horizon, turns the camera around the view's center instead, as a
/// tilt without an anchor does: a gesture's step, and each frame of a
/// command.
#[test]
fn a_tilt_that_carries_its_anchor_into_the_sky_turns_around_the_center() {
    let run_loop = Queue::<()>::new();
    let tilt = |anchored: bool| {
        let (mut map, _) = map_over(Some(&slope()), &run_loop);
        map.jump_to(&CameraOptions { pitch: Some(70.0), ..Default::default() });
        let anchor = ScreenCoordinate::new(330.0, horizon(&map) + 10.0);
        let delta = CameraDelta {
            pitch: Some(10.0),
            anchor: anchored.then_some(anchor),
            phase: GesturePhase::Begin,
            ..Default::default()
        };
        map.apply_camera_delta(&delta).unwrap();
        assert!(anchor.y < horizon(&map), "the tilt carries the anchor into the sky");
        map.transform_state().camera_options(None)
    };
    assert_eq!(tilt(false), tilt(true), "a gesture's tilt");

    // A command's frames hold the anchor until it reaches the sky, and then
    // leave the center where they held it.
    let (mut map, clock) = map_over(Some(&slope()), &run_loop);
    map.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    let anchor = ScreenCoordinate::new(330.0, horizon(&map) + 150.0);
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { pitch: Some(85.0), anchor: Some(anchor), ..Default::default() },
        animation: AnimationOptions {
            duration: Some(Duration::from_secs(1)),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
        ..Default::default()
    });
    let mut center = map.transform_state().view().center(WrapMode::Unwrapped);
    let (mut held, mut skyward) = (0, 0);
    frames(&mut map, clock, Duration::from_secs(1), |map, elapsed| {
        let now = map.transform_state().view().center(WrapMode::Unwrapped);
        if anchor.y < horizon(map) {
            let moved = (world(&now) - world(&center)).length() / PIXEL;
            assert!(moved < 1e-6, "{elapsed:?}: the anchor is in the sky, and the center moved {moved} px");
            skyward += 1;
        } else {
            held += 1;
        }
        center = now;
    });
    assert!(held > 5 && skyward > 5, "{held} frames held the anchor, and {skyward} didn't");
}

/// Under a roll, where the horizon isn't a row of the screen, a turn around
/// a point whose line of sight doesn't descend to the ground turns around
/// the view's center, as a turn without an anchor does, and a turn around a
/// point on the ground holds that ground under it.
#[test]
fn a_turn_under_a_roll_holds_only_a_point_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let rolled = CameraOptions { pitch: Some(80.0), roll: Some(90.0), ..Default::default() };
    let turn = |anchor: Option<DVec2>| {
        let (mut map, _) = map_over(Some(&slope()), &run_loop);
        map.jump_to(&rolled);
        let delta = CameraDelta {
            bearing: Some(15.0),
            anchor: anchor.map(|anchor| ScreenCoordinate::new(anchor.x, anchor.y)),
            phase: GesturePhase::Begin,
            ..Default::default()
        };
        map.apply_camera_delta(&delta).unwrap();
        map
    };
    let (map, _) = map_over(Some(&slope()), &run_loop);
    let mut map = map;
    map.jump_to(&rolled);
    let view = map.transform_state().view();
    let descends = |point: DVec2| {
        let (near, far) = view.line_of_sight(&view_point(view, point)).unwrap();
        far.z < near.z
    };
    // The roll puts the sky on one side of the screen and the ground on the
    // other.
    let (left, right) = (DVec2::new(20.0, 480.0), DVec2::new(492.0, 480.0));
    let (sky, ground) = if descends(left) { (right, left) } else { (left, right) };
    assert!(!descends(sky) && descends(ground), "the roll turns the horizon upright");
    assert!(view.ground_under(&view_point(view, sky), &*map.ground().unwrap()).is_none());
    let horizon_row = view.padding().center_in(view.size()).y - view.mercator_horizon();
    assert!(sky.y > horizon_row, "below the row of the unrolled horizon");

    let camera = |map: &Map| map.transform_state().camera_options(None);
    assert_eq!(camera(&turn(None)), camera(&turn(Some(sky))), "a turn around the sky");

    let grabbed = ground_under(&map, ground);
    let map = turn(Some(ground));
    let error = on_screen(&map, grabbed.world, grabbed.height).distance(ground);
    assert!(error < 0.01, "the ground moved {error} px from the anchor");
}
