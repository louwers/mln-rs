//! How a motion over a clamped center comes to rest: whatever moved the
//! camera last, a command's last frame, a momentum's end or a gesture's
//! end, leaves the center on the ground under it, moved there along the
//! view with the eye where it is, before the motion's last camera event,
//! so the camera at rest moves no more.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3};
use mln_map_types::{ConstrainMode, Elevation, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
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
