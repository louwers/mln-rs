//! How a motion over a clamped center comes to rest, on the ground under
//! the center, before the motion's last camera event, so the camera at rest
//! moves no more: a command that completes ends at the camera it asked for,
//! its altitude on the ground there, and anything else that brings the
//! camera to rest moves the center onto the ground along the view, with the
//! eye where it is, where the limits allow it.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3};
use mln_map_types::{CameraLimits, ConstrainMode, Elevation, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::projection::Projection;
use mln_util::queue::Queue;
use mln_util::size::Size;
use mln_util::time::Instant;

use super::ground::{Slope, flat, ground_event, report, world};
use super::terrain_camera::{FRAME, frames, map_over, pixel, start};
use super::terrain_gestures::{Hill, meters};
use crate::transform::{TerrainGround, Transform, TransformEvent};
use crate::{
    AnimationOptions, CameraChangeSource, CameraDelta, CameraMomentum, CameraOptions, CameraUpdate, CameraUpdateMode,
    GesturePhase, Map, MapEvent, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumGlide,
};

/// An ease over a second, with an easing of its own so that it doesn't fly.
fn eased() -> AnimationOptions {
    AnimationOptions {
        duration: Some(Duration::from_secs(1)),
        easing: Some(DEFAULT_TRANSITION_EASE),
        ..Default::default()
    }
}

/// The location `north` meters north of the start.
fn north_of_start(north: f64) -> LatLng {
    let point = world(&start()) - DVec2::new(0.0, north / meters());
    Projection::unproject(&geometry::Point::new(point.x, point.y), 1.0 / 512.0, WrapMode::Unwrapped).unwrap()
}

/// What a camera shows: its eye, and the ground drawn under three points of
/// the screen (the center, and points left and right of it), with where
/// that ground draws.
struct Picture {
    eye: DVec3,
    ground: Vec<(DVec2, f64, DVec2)>,
}

impl Picture {
    fn of(view: &View, ground: &dyn Elevation) -> Picture {
        let ground = [(256.0, 256.0), (100.0, 200.0), (412.0, 200.0)]
            .into_iter()
            .map(|(x, y)| {
                let point = ScreenCoordinate::new(x, f64::from(view.size().height) - y);
                let hit = view.ground_under(&point, ground).expect("ground under the point");
                (hit.world, hit.height, pixel(view, hit.world, hit.height))
            })
            .collect();
        Picture { eye: view.eye_position(), ground }
    }

    /// Checks that `view`'s eye stands within `eye_tolerance` meters of
    /// this one's, that the ground draws within `tolerance` px of where it
    /// drew, and that the center stands on `ground`.
    fn assert_kept_by(&self, view: &View, ground: &dyn Elevation, eye_tolerance: f64, tolerance: f64, what: &str) {
        let moved = view.eye_position() - self.eye;
        let moved = DVec3::new(moved.x * meters(), moved.y * meters(), moved.z).length();
        assert!(moved < eye_tolerance, "{what}: the eye moved {moved} m");
        for &(world, height, drawn) in &self.ground {
            let now = pixel(view, world, height);
            assert!(now.distance(drawn) < tolerance, "{what}: the ground at {drawn} moved to {now}");
        }
        let center = view.center(WrapMode::Wrapped);
        assert_eq!(ground.at(center).unwrap(), view.center_altitude(), "{what}: the center stands on the ground");
    }
}

/// Checks that the next update leaves `map`'s camera where it is.
fn assert_at_rest(map: &mut Map, what: &str) {
    let view = map.transform_state().view().clone();
    map.trigger_repaint();
    assert_eq!(view, *map.transform_state().view(), "{what}: the next update moves the camera");
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

/// A command cancelled halfway up a hill, its altitude eased toward the
/// ground beyond it, ends with its center moved onto the hill along the
/// view: neither the eye nor the picture moves, the cancel's
/// `CameraDidChange` is the last camera event, and the camera at rest moves
/// no more.
#[test]
fn a_cancelled_command_ends_on_the_ground_with_the_eye_where_it_was() {
    let run_loop = Queue::<()>::new();
    let hill: Arc<dyn Elevation> = Arc::new(Hill::new(8000.0));
    let (mut map, clock) = map_over(Some(&hill), &run_loop);
    assert_eq!(0.0, map.transform_state().view().center_altitude());
    let destination = north_of_start(16000.0);
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { center: Some(destination), ..Default::default() },
        animation: eased(),
        ..Default::default()
    });
    // Cancelled once the center is a kilometer under the hill.
    let mut before = None;
    frames(&mut map, clock, Duration::from_secs(1), |map, _| {
        let view = map.transform_state().view();
        let off = hill.at(view.center(WrapMode::Wrapped)).unwrap() - view.center_altitude();
        if before.is_none() && off > 1000.0 {
            before = Some(Picture::of(view, &*hill));
            camera_events(map);
            map.cancel_transitions();
        }
    });
    let before = before.expect("the ease passes over the hill");
    before.assert_kept_by(map.transform_state().view(), &*hill, 1e-3, 0.05, "cancelled");
    assert_eq!(vec![(true, CameraChangeSource::Command)], camera_events(&mut map));
    assert_at_rest(&mut map, "cancelled");
    assert_eq!(Vec::<(bool, CameraChangeSource)>::new(), camera_events(&mut map));
}

/// A glide that lasts `seconds` from `speed`, along a curve that ends at
/// rest.
fn decay(seconds: u64, speed: f64) -> MomentumDecay {
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

/// A gesture's step: a pan by `offset`, or a turn by `bearing`.
fn step(phase: GesturePhase, offset: Option<(f64, f64)>, bearing: Option<f64>) -> CameraDelta {
    CameraDelta { offset: offset.map(|(x, y)| ScreenCoordinate::new(x, y)), bearing, phase, ..Default::default() }
}

/// Fingers that go down again while a command eases the center it took from
/// a gesture's glide own the altitude from then on: the command no longer
/// eases it, and the gesture holds it where it was. Whether they lift
/// before the command ends or after, the motion ends on the ground with the
/// eye where its last frame put it, and the camera at rest moves no more.
#[test]
fn a_second_gesture_during_a_command_holds_the_altitude_until_the_motion_ends() {
    let destination = LatLng::new(20.0, 10.02).unwrap();
    // Ground at sea level under the start that rises half a meter for each
    // meter east.
    let ground: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 0.0, rise: 2e7 });
    for lifted in [Duration::from_millis(608), Duration::from_millis(1500)] {
        let run_loop = Queue::<()>::new();
        let (mut map, clock) = map_over(Some(&ground), &run_loop);
        map.apply_camera_delta(&step(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
        let momentum = CameraMomentum {
            pan: Some(MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay: decay(1, 1000.0) }),
            ..Default::default()
        };
        map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..step(GesturePhase::End, None, None) })
            .unwrap();
        let started = clock + Duration::from_millis(96);
        map.set_frame_time(Some(started));
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Ease,
            camera: CameraOptions { center: Some(destination), ..Default::default() },
            animation: eased(),
            ..Default::default()
        });
        let down = Duration::from_millis(304);
        let mut held = None;
        let last = Duration::from_millis(999);
        frames(&mut map, started, last, |map, elapsed| {
            let altitude = map.transform_state().view().center_altitude();
            if elapsed == down {
                map.apply_camera_delta(&step(GesturePhase::Update, None, Some(10.0))).unwrap();
                held = Some(map.transform_state().view().center_altitude());
            } else if let Some(held) = held {
                assert_eq!(held, altitude, "lifted at {lifted:?}: the altitude at {elapsed:?}");
            }
            if elapsed == lifted {
                map.apply_camera_delta(&step(GesturePhase::End, None, None)).unwrap();
            }
        });
        let held = held.unwrap();
        let end = ground.at(destination).unwrap();
        assert!(end - held > 100.0, "lifted at {lifted:?}: held {held} m, under the ground at {end} m");

        // The command's last millisecond.
        let before = Picture::of(map.transform_state().view(), &*ground);
        map.set_frame_time(Some(started + Duration::from_secs(1)));
        assert!(!map.snapshot().transition_in_progress);
        if lifted < Duration::from_secs(1) {
            before.assert_kept_by(map.transform_state().view(), &*ground, 0.05, 0.05, "the command's end");
        } else {
            assert_eq!(held, map.transform_state().view().center_altitude(), "the fingers are down");
            map.set_frame_time(Some(started + lifted));
            map.apply_camera_delta(&step(GesturePhase::End, None, None)).unwrap();
            before.assert_kept_by(map.transform_state().view(), &*ground, 0.05, 0.05, "the gesture's end");
        }
        assert!(!map.snapshot().gesture_in_progress);
        assert_at_rest(&mut map, &format!("lifted at {lifted:?}"));
    }
}

/// Ground about a quarter of a meter up for each meter east of the start,
/// 1000 m high there.
fn steep() -> Arc<dyn Elevation> {
    Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise: 1e7 })
}

/// A transform 512 by 512 over `ground`, which clamps its center, at rest at
/// the start at zoom 12, pitched `pitch`, its frame clock an hour ahead.
fn transform_over(ground: &Arc<dyn Elevation>, pitch: f64) -> (Transform, Instant) {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let mut limits = *transform.state().limits();
    limits.max_pitch = 85f64.to_radians();
    transform.set_limits(limits).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(ground)), clamped: true }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    transform.jump_to(&CameraOptions {
        center: Some(start()),
        zoom: Some(12.0),
        pitch: Some(pitch),
        bearing: Some(-30.0),
        ..Default::default()
    });
    transform.follow_terrain();
    (transform, clock)
}

/// Steps `transform`'s frames from `from` to `until`, as the map updates:
/// the frame, then, while a command runs, the ground; calls `each` before
/// each frame.
fn transform_frames(
    transform: &mut Transform,
    from: Instant,
    until: Instant,
    mut each: impl FnMut(&mut Transform, Instant),
) {
    let mut now = from;
    while now < until {
        now = (now + FRAME).min(until);
        transform.set_frame_time(Some(now));
        each(transform, now);
        transform.update_transitions(Some(now));
        if transform.in_transition() {
            transform.follow_terrain();
        }
    }
}

/// An anchored zoom over a steady slope, under which a separate command
/// changes the pitch, solves its end again in the camera the pitch leaves:
/// its last frame stands on the ground at the zoom it asked for, without a
/// move onto the ground to finish it.
#[test]
fn an_anchored_zoom_under_a_pitch_change_ends_on_the_ground() {
    let ground = steep();
    let (mut transform, clock) = transform_over(&ground, 60.0);
    let zoom = transform.view().zoom() + 1.0;
    transform.ease_to(
        &CameraOptions { zoom: Some(zoom), anchor: Some(ScreenCoordinate::new(120.0, 400.0)), ..Default::default() },
        &eased(),
    );
    let pitched = clock + Duration::from_millis(208);
    transform_frames(&mut transform, clock, clock + Duration::from_secs(1), |transform, now| {
        if now == pitched {
            transform.ease_to(
                &CameraOptions { pitch: Some(70.0), ..Default::default() },
                &AnimationOptions { duration: Some(Duration::from_millis(500)), ..eased() },
            );
        }
    });
    assert!(!transform.in_transition());
    let view = transform.view();
    assert_eq!(70.0, view.pitch().to_degrees().round());
    assert_eq!(zoom, view.zoom(), "the zoom it asked for");
    let off = view.center_altitude() - ground.at(view.center(WrapMode::Wrapped)).unwrap();
    assert!(off.abs() < 1e-6, "the last frame stands {off} m off the ground");
}

/// A command whose anchor first leaves the plane it holds on at its last
/// frame, as after a long frame, gives up its anchor there and still ends
/// on the ground, with its events in order: its end, then its
/// `CameraDidChange`, and nothing after.
#[test]
fn an_anchor_dropped_on_the_last_frame_ends_on_the_ground() {
    let ground = steep();
    let (mut transform, clock) = transform_over(&ground, 60.0);
    let view = transform.view();
    let horizon = view.padding().center_in(view.size()).y - view.mercator_horizon();
    transform.ease_to(
        &CameraOptions {
            pitch: Some(85.0),
            anchor: Some(ScreenCoordinate::new(330.0, horizon + 150.0)),
            ..Default::default()
        },
        &eased(),
    );
    let center = transform.view().center(WrapMode::Unwrapped);
    transform.set_frame_time(Some(clock + FRAME));
    transform.update_transitions(Some(clock + FRAME));
    transform.follow_terrain();
    assert_ne!(center, transform.view().center(WrapMode::Unwrapped), "the first frame holds the anchor");
    transform.drain_events().for_each(drop);

    let end = clock + Duration::from_secs(1);
    transform.set_frame_time(Some(end));
    transform.update_transitions(Some(end));
    assert!(!transform.in_transition());
    let view = transform.view().clone();
    let off = view.center_altitude() - ground.at(view.center(WrapMode::Wrapped)).unwrap();
    assert!(off.abs() < 1e-3, "the command ends {off} m off the ground");
    let events: Vec<_> = transform
        .drain_events()
        .map(|event| match event {
            TransformEvent::CameraWillChange(_) => "will",
            TransformEvent::CameraIsChanging => "changing",
            TransformEvent::CameraDidChange(_) => "did",
            TransformEvent::CommandEnded(_) => "ended",
        })
        .collect();
    assert_eq!(vec!["ended", "did"], events);
    assert!(!transform.follow_terrain(), "at rest");
    assert_eq!(view, *transform.view());
}

/// Terrain that goes during an anchored ease over a clamped center takes
/// the center to sea level and holds the anchor's location at sea level in
/// one step: the location under the anchor once the terrain went stays
/// under it through the rest of the ease.
#[test]
fn a_clamped_anchored_ease_that_loses_its_terrain_holds_the_flat_map() {
    let height = 1000.0;
    let ground = flat(Some(height), height);
    let (mut transform, clock) = transform_over(&ground, 50.0);
    assert_eq!(height, transform.view().center_altitude());
    let anchor = ScreenCoordinate::new(120.0, 400.0);
    transform.ease_to(
        &CameraOptions { zoom: Some(13.0), bearing: Some(20.0), anchor: Some(anchor), ..Default::default() },
        &eased(),
    );
    let mut held = None::<LatLng>;
    let mut checked = 0;
    let mut now = clock;
    while now < clock + Duration::from_secs(1) {
        now += FRAME;
        if now >= clock + Duration::from_millis(500) && held.is_none() {
            transform.set_terrain(None);
            transform.follow_terrain();
            assert_eq!(0.0, transform.view().center_altitude(), "the center goes to sea level");
            held = Some(transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap());
        }
        transform.set_frame_time(Some(now));
        transform.update_transitions(Some(now));
        if let Some(held) = held {
            let point = transform.lat_lng_to_screen_coordinate(&held);
            let off = (point.x - anchor.x).hypot(point.y - anchor.y);
            assert!(off < 0.01, "the ground at sea level is {off} px from the anchor");
            checked += 1;
        }
    }
    assert!(checked > 10);
}

/// The camera a command ends in on the ground stands still on newer ground
/// that differs only away from its center: the move onto the ground at its
/// end happens once, as the motion ends.
#[test]
fn a_command_ended_on_the_ground_stands_still_on_newer_ground() {
    let run_loop = Queue::<()>::new();
    let hill: Arc<dyn Elevation> = Arc::new(Hill::new(8000.0));
    let (mut map, clock) = map_over(Some(&hill), &run_loop);
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { center: Some(north_of_start(16000.0)), ..Default::default() },
        animation: eased(),
        ..Default::default()
    });
    frames(&mut map, clock, Duration::from_secs(1), |map, _| {
        let view = map.transform_state().view();
        if hill.at(view.center(WrapMode::Wrapped)).unwrap() - view.center_altitude() > 1000.0 {
            map.cancel_transitions();
        }
    });
    let view = map.transform_state().view().clone();
    assert!(view.center_altitude() > 500.0, "the cancel moved the center onto the hill");
    // The same height and floor at the center, and none elsewhere.
    let center = view.center(WrapMode::Wrapped);
    let newer = flat(Some(hill.at(center).unwrap()), hill.min_at(center).unwrap());
    report(&mut map, ground_event(&newer));
    assert_eq!(view, *map.transform_state().view());
}

/// Ground `height` meters high at the start that rises `east` meters for
/// each meter east and `north` for each meter north, and is rough by up to
/// `rough` meters, as a DEM's rounding leaves it.
#[derive(Debug)]
struct Incline {
    height: f64,
    east: f64,
    north: f64,
    rough: f64,
}

impl Incline {
    fn steady(east: f64, north: f64) -> Arc<dyn Elevation> {
        Arc::new(Incline { height: 1000.0, east, north, rough: 0.0 })
    }

    fn height(&self, at: DVec2) -> f64 {
        let moved = (at - world(&start())) * meters();
        let rough = self.rough * ((at.x * 1e9 + at.y * 3e9).sin() * 43_758.545_3).fract();
        self.height + self.east * moved.x - self.north * moved.y + rough
    }
}

impl Elevation for Incline {
    fn at(&self, location: LatLng) -> Option<f64> {
        Some(self.height(world(&location)))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(self.height)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        Some(self.height(world))
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((-1e5, 1e5))
    }
}

/// Where `ground` draws under `point` (y down) of `view`: the ground there,
/// and its height.
fn ground_under(view: &View, ground: &dyn Elevation, point: ScreenCoordinate) -> (DVec2, f64) {
    let y_up = ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y);
    let hit = view.ground_under(&y_up, ground).expect("ground under the point");
    (hit.world, hit.height)
}

/// Checks that the ground `held` draws within 0.01 px of `point` (y down)
/// on `view`.
fn assert_holds(view: &View, held: (DVec2, f64), point: ScreenCoordinate, what: &str) {
    let drawn = pixel(view, held.0, held.1);
    let off = drawn.distance(DVec2::new(point.x, f64::from(view.size().height) - point.y));
    assert!(off < 0.01, "{what}: the ground under the anchor moved {off} px");
}

/// Checks that `view`'s center stands exactly on `ground`.
fn assert_on_ground(view: &View, ground: &dyn Elevation, what: &str) {
    let at = ground.at(view.center(WrapMode::Wrapped)).unwrap();
    assert_eq!(at, view.center_altitude(), "{what}: the center stands on the ground");
}

/// Starts `camera` as a command of `mode` over a second.
fn run(map: &mut Map, mode: CameraUpdateMode, camera: CameraOptions, animation: AnimationOptions) {
    map.update_camera(&CameraUpdate { mode, camera, animation, ..Default::default() });
}

/// Every kind of command that completes over terrain, alone or beside
/// another command that changes the pitch or the bearing, ends at exactly
/// the center and zoom it asked for, its altitude on the ground there, and
/// the next update moves nothing. An anchored command ends with the ground
/// it grabbed under its anchor.
#[test]
fn a_command_that_completes_ends_at_the_camera_it_asked_for() {
    let destination = LatLng::new(20.02, 10.05).unwrap();
    let anchor = ScreenCoordinate::new(120.0, 400.0);
    let to = CameraOptions { center: Some(destination), zoom: Some(13.0), ..Default::default() };
    let around = CameraOptions { zoom: Some(13.0), anchor: Some(anchor), ..Default::default() };
    let commands = [
        ("an ease", CameraUpdateMode::Ease, to),
        ("a flight", CameraUpdateMode::Fly, to),
        ("a jump", CameraUpdateMode::Jump, to),
        ("an anchored ease", CameraUpdateMode::Ease, CameraOptions { bearing: Some(20.0), ..around }),
        ("an anchored zoom", CameraUpdateMode::Ease, around),
    ];
    // Beside it: nothing, the pitch over the same second, or the bearing
    // from 208 ms to 708 ms.
    let pitch = CameraOptions { pitch: Some(55.0), ..Default::default() };
    let bearing = CameraOptions { bearing: Some(-25.0), ..Default::default() };
    let besides = [("alone", None), ("beside a pitch", Some((pitch, 0))), ("beside a turn", Some((bearing, 208)))];
    for (name, mode, camera) in commands {
        for (beside, other) in besides {
            if camera.bearing.is_some() && other.is_some_and(|(other, _)| other.bearing.is_some()) {
                continue;
            }
            let what = format!("{name} {beside}");
            let run_loop = Queue::<()>::new();
            let ground = Incline::steady(0.25, 0.1);
            let (mut map, clock) = map_over(Some(&ground), &run_loop);
            let grabbed = ground_under(map.transform_state().view(), &*ground, anchor);
            let other_animation = |delay| AnimationOptions {
                duration: Some(Duration::from_millis(if delay == 0 { 1000 } else { 500 })),
                ..eased()
            };
            if let Some((other, 0)) = other {
                run(&mut map, CameraUpdateMode::Ease, other, other_animation(0));
            }
            run(&mut map, mode, camera, eased());
            frames(&mut map, clock, Duration::from_secs(1), |map, elapsed| {
                if let Some((other, delay)) = other.filter(|&(_, delay)| delay != 0)
                    && elapsed == Duration::from_millis(delay)
                {
                    run(map, CameraUpdateMode::Ease, other, other_animation(delay));
                }
            });
            assert!(!map.snapshot().transition_in_progress, "{what}");
            let view = map.transform_state().view().clone();
            assert_eq!(13.0, view.zoom(), "{what}: the zoom it asked for");
            if camera.anchor.is_some() {
                assert_holds(&view, grabbed, anchor, &what);
            } else {
                let center = view.center(WrapMode::Wrapped);
                assert!((center.latitude() - destination.latitude()).abs() < 1e-9, "{what}: {center:?}");
                assert!((center.longitude() - destination.longitude()).abs() < 1e-9, "{what}: {center:?}");
            }
            assert_on_ground(&view, &*ground, &what);
            assert_at_rest(&mut map, &what);
        }
    }
}

/// An anchored zoom over a slope that a DEM's rounding leaves rough by a
/// fraction of a millimeter, over which its end can't be solved to a
/// micrometer, ends at the zoom it asked for, with the ground it grabbed
/// under its anchor and its center on the ground.
#[test]
fn an_anchored_zoom_on_rough_ground_ends_where_it_asked() {
    let ground: Arc<dyn Elevation> = Arc::new(Incline { height: 1000.0, east: 0.25, north: 0.1, rough: 5e-5 });
    for anchor in [(120.0, 400.0), (400.0, 300.0), (60.0, 480.0), (256.0, 450.0), (450.0, 200.0)] {
        let anchor = ScreenCoordinate::new(anchor.0, anchor.1);
        let what = format!("around {anchor:?}");
        let run_loop = Queue::<()>::new();
        let (mut map, clock) = map_over(Some(&ground), &run_loop);
        let grabbed = ground_under(map.transform_state().view(), &*ground, anchor);
        let camera = CameraOptions { zoom: Some(13.0), anchor: Some(anchor), ..Default::default() };
        run(&mut map, CameraUpdateMode::Ease, camera, eased());
        frames(&mut map, clock, Duration::from_secs(1), |_, _| {});
        let view = map.transform_state().view().clone();
        assert_eq!(13.0, view.zoom(), "{what}: the zoom it asked for");
        assert_holds(&view, grabbed, anchor, &what);
        assert_on_ground(&view, &*ground, &what);
        assert_at_rest(&mut map, &what);
    }
}

/// An anchored zoom with a pitch change that ends with it solves its end in
/// the camera of each frame, the pitch that frame reaches included: sampled
/// halfway and then at the shared end, it ends at the zoom it asked for,
/// with the ground it grabbed under its anchor and its center on the ground.
#[test]
fn an_anchored_zoom_ending_with_a_pitch_change_solves_its_end_in_the_frame() {
    let ground = steep();
    let (mut transform, clock) = transform_over(&ground, 60.0);
    let anchor = ScreenCoordinate::new(120.0, 400.0);
    let grabbed = ground_under(transform.view(), &*ground, anchor);
    transform.ease_to(&CameraOptions { pitch: Some(70.0), ..Default::default() }, &eased());
    transform.ease_to(&CameraOptions { zoom: Some(13.0), anchor: Some(anchor), ..Default::default() }, &eased());
    for elapsed in [500, 1000] {
        let now = clock + Duration::from_millis(elapsed);
        transform.set_frame_time(Some(now));
        transform.update_transitions(Some(now));
        if transform.in_transition() {
            transform.follow_terrain();
        }
    }
    assert!(!transform.in_transition());
    let view = transform.view().clone();
    assert_eq!(13.0, view.zoom(), "the zoom it asked for");
    assert_eq!(70.0, view.pitch().to_degrees().round());
    assert_holds(&view, grabbed, anchor, "at the end");
    assert_on_ground(&view, &*ground, "at the end");
    assert!(!transform.follow_terrain(), "at rest");
}

/// A transform under [`ConstrainMode::Screen`], 512 by 512, over ground
/// that rises to the north, whose bounds end 3 km north of the start: at
/// rest at the start at zoom 13, pitched 50 degrees.
fn transform_under_screen_bounds(ground: &Arc<dyn Elevation>) -> Transform {
    let mut transform = Transform::new(ConstrainMode::Screen, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let north = north_of_start(3000.0);
    let bounds = LatLngBounds::hull(
        LatLng::new(start().latitude() - 1.0, start().longitude() - 1.0).unwrap(),
        LatLng::new(north.latitude(), start().longitude() + 1.0).unwrap(),
    );
    transform.set_limits(CameraLimits { bounds, ..*transform.state().limits() }).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(ground)), clamped: true }));
    transform.set_frame_time(Some(Instant::now() + Duration::from_secs(3600)));
    transform.jump_to(&CameraOptions {
        center: Some(start()),
        zoom: Some(13.0),
        pitch: Some(50.0),
        ..Default::default()
    });
    transform.follow_terrain();
    transform
}

/// Checks that `transform`'s bounds take its camera as it is.
fn assert_within_screen_bounds(transform: &Transform, what: &str) {
    let view = transform.view();
    let center = view.center(WrapMode::Unwrapped);
    let mut camera = CameraOptions { center: Some(center), ..Default::default() };
    let mut zoom = view.zoom();
    transform.state().constrain_camera_and_zoom_to_bounds(&mut camera, &mut zoom);
    assert_eq!((Some(center), view.zoom()), (camera.center, zoom), "{what}: the bounds move the camera");
}

/// A zoom-only jump that the bounds under [`ConstrainMode::Screen`] move
/// south of its center ends there, at the zoom it asked for, its altitude
/// on the ground, within the bounds.
#[test]
fn a_jump_that_screen_bounds_move_ends_within_them() {
    let ground = Incline::steady(0.0, 0.25);
    let mut transform = transform_under_screen_bounds(&ground);
    transform.jump_to(&CameraOptions { zoom: Some(12.0), ..Default::default() });
    let view = transform.view();
    assert_eq!(12.0, view.zoom());
    assert!(view.center(WrapMode::Wrapped).latitude() < start().latitude(), "the bounds move the center south");
    assert_on_ground(view, &*ground, "the jump");
    assert_within_screen_bounds(&transform, "the jump");
    assert!(!transform.follow_terrain(), "at rest");
}

/// A gesture that ends over ground lower than the altitude it held, at the
/// bounds' north edge under [`ConstrainMode::Screen`], where moving the
/// center along the view onto the ground would take the view past the
/// bounds, ends with its center where it was, on the ground there.
#[test]
fn a_gesture_end_that_screen_bounds_refuse_stands_the_center_on_the_ground() {
    let ground = Incline::steady(0.0, 0.25);
    let mut transform = transform_under_screen_bounds(&ground);
    transform.jump_to(&CameraOptions { zoom: Some(12.0), ..Default::default() });
    let center = transform.view().center(WrapMode::Unwrapped);
    transform.apply_camera_delta(&step(GesturePhase::Begin, None, None)).unwrap();
    let lower: Arc<dyn Elevation> = Arc::new(Incline { height: 500.0, east: 0.0, north: 0.25, rough: 0.0 });
    transform.set_terrain(Some(TerrainGround { elevation: Some(Arc::clone(&lower)), clamped: true }));
    transform.follow_terrain();
    assert!(transform.view().center_altitude() > lower.at(center).unwrap() + 400.0, "the gesture holds");
    transform.apply_camera_delta(&step(GesturePhase::End, None, None)).unwrap();
    let view = transform.view();
    assert_eq!(center, view.center(WrapMode::Unwrapped));
    assert_eq!(12.0, view.zoom());
    assert_on_ground(view, &*lower, "the gesture's end");
    assert_within_screen_bounds(&transform, "the gesture's end");
}

/// Fingers that go down while a command eases only the altitude take the
/// altitude from it, as from any command: the gesture holds the altitude
/// the camera has, and when the fingers lift, nothing moves the camera on
/// and the motion ends on the ground with the eye where it was.
#[test]
fn fingers_down_take_the_altitude_from_an_explicit_altitude_ease() {
    let run_loop = Queue::<()>::new();
    let ground = flat(Some(1000.0), 1000.0);
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    assert_eq!(1000.0, map.transform_state().view().center_altitude());
    run(
        &mut map,
        CameraUpdateMode::Ease,
        CameraOptions { center_altitude: Some(3000.0), ..Default::default() },
        eased(),
    );
    let mut held = None;
    frames(&mut map, clock, Duration::from_millis(608), |map, elapsed| {
        let altitude = map.transform_state().view().center_altitude();
        if elapsed == Duration::from_millis(304) {
            map.apply_camera_delta(&step(GesturePhase::Update, None, Some(10.0))).unwrap();
            held = Some(map.transform_state().view().center_altitude());
        } else if let Some(held) = held {
            assert_eq!(held, altitude, "the fingers are down at {elapsed:?}");
        }
    });
    let held = held.unwrap();
    assert!(held > 1100.0, "the command eased the altitude to {held} m");
    assert!(!map.snapshot().transition_in_progress, "the gesture took the altitude");
    let eye = map.transform_state().view().eye_position();
    map.apply_camera_delta(&step(GesturePhase::End, None, None)).unwrap();
    map.trigger_repaint();
    let view = map.transform_state().view();
    let moved = view.eye_position() - eye;
    let moved = DVec3::new(moved.x * meters(), moved.y * meters(), moved.z).length();
    assert!(moved < 1e-3, "the eye moved {moved} m");
    assert_on_ground(view, &*ground, "the gesture's end");
    assert_at_rest(&mut map, "the gesture's end");
}

/// Terrain that goes during an anchored ease over a clamped center before
/// it drew any ground takes the center to sea level, as once it had: the
/// altitude the command was given, which the ground would have overruled,
/// doesn't stay.
#[test]
fn terrain_that_goes_before_any_ground_takes_a_clamped_center_to_sea_level() {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_terrain(Some(TerrainGround { elevation: None, clamped: true }));
    let clock = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(clock));
    transform.jump_to(&CameraOptions {
        center: Some(start()),
        zoom: Some(12.0),
        pitch: Some(50.0),
        center_altitude: Some(1000.0),
        ..Default::default()
    });
    transform.follow_terrain();
    assert_eq!(1000.0, transform.view().center_altitude(), "the ground is loading");
    transform.ease_to(
        &CameraOptions {
            zoom: Some(13.0),
            center_altitude: Some(1000.0),
            anchor: Some(ScreenCoordinate::new(120.0, 400.0)),
            ..Default::default()
        },
        &eased(),
    );
    let mut gone = false;
    transform_frames(&mut transform, clock, clock + Duration::from_secs(1), |transform, now| {
        if now >= clock + Duration::from_millis(500) && !gone {
            transform.set_terrain(None);
            transform.follow_terrain();
            gone = true;
        }
        if gone {
            assert_eq!(0.0, transform.view().center_altitude(), "at sea level");
        }
    });
    assert!(gone && !transform.in_transition());
    assert_eq!(0.0, transform.view().center_altitude());
}
