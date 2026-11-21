//! How gestures move the camera over the terrain: a gesture holds the
//! center's altitude, lifted over ground that reaches above the eye, and its
//! end moves the center onto the ground along the view, with the eye and
//! the picture where they were.

use std::sync::Arc;
use std::time::Duration;

use glam::{DVec2, DVec3};
use mln_map_types::{ConstrainMode, Elevation, View, ViewportMode};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::projection::Projection;
use mln_util::queue::Queue;
use mln_util::size::Size;

use super::ground::{Slope, assert_near, flat, ground, ground_event, report, world};
use super::terrain_camera::{frames, map_over, pixel, start};
use crate::transform::{TerrainGround, Transform, TransformEvent};
use crate::{
    AnchoredMomentum, AnimationOptions, BoundOptions, CameraChangeSource, CameraDelta, CameraMomentum, CameraOptions,
    CameraUpdate, CameraUpdateMode, GesturePhase, Map, MapEvent, MomentumDecay, MomentumDecayModel, MomentumEasing,
    MomentumGlide, MomentumSnap,
};

/// Ground at sea level under the start that rises about half a meter for
/// each meter east, so that a gesture that holds the start's altitude ends
/// above or below the ground in front of it.
fn slope() -> Arc<dyn Elevation> {
    Arc::new(Slope { x: world(&start()).x, height: 0.0, rise: 2e7 })
}

/// Where the ground at `points` draws on `view`.
fn pixels(view: &View, ground: &dyn Elevation, points: &[DVec2]) -> Vec<DVec2> {
    points.iter().map(|&point| pixel(view, point, ground.surface_at(point).unwrap())).collect()
}

/// The ground under three points of the screen: the center, and points left
/// and right of it.
fn ground_points(map: &Map) -> Vec<DVec2> {
    let view = map.transform_state().view();
    [(256.0, 256.0), (100.0, 200.0), (412.0, 200.0)]
        .into_iter()
        .map(|(x, y)| {
            let point = ScreenCoordinate::new(x, f64::from(view.size().height) - y);
            view.ground_under(&point, &*map.ground().unwrap()).unwrap().world
        })
        .collect()
}

/// Checks that `map`'s eye stands within `eye_tolerance` meters of `eye`,
/// that the ground at `points` draws within 0.05 px of `drawn`, and that
/// the center stands on the ground, at rest: the next update moves nothing.
fn assert_still_at(
    map: &mut Map,
    ground: &dyn Elevation,
    eye: DVec3,
    eye_tolerance: f64,
    points: &[DVec2],
    drawn: &[DVec2],
) {
    let view = map.transform_state().view().clone();
    let meters = Projection::altitude_meters_per_pixel(start().latitude(), 0.0) * 512.0;
    let moved = view.eye_position() - eye;
    let moved = DVec3::new(moved.x * meters, moved.y * meters, moved.z).length();
    assert!(moved < eye_tolerance, "the eye moved {moved} m");
    for (point, before) in pixels(&view, ground, points).iter().zip(drawn) {
        assert!(point.distance(*before) < 0.05, "the ground at {before} moved to {point}");
    }
    assert_eq!(
        ground.at(view.center(WrapMode::Wrapped)).unwrap(),
        view.center_altitude(),
        "the center stands on the ground"
    );
    map.trigger_repaint();
    assert_eq!(view, *map.transform_state().view(), "the next update moves nothing");
}

/// A gesture's step.
fn delta(phase: GesturePhase, offset: Option<(f64, f64)>, scale: Option<f64>) -> CameraDelta {
    CameraDelta {
        offset: offset.map(|(x, y)| ScreenCoordinate::new(x, y)),
        scale,
        anchor: scale.map(|_| ScreenCoordinate::new(100.0, 150.0)),
        phase,
        ..Default::default()
    }
}

/// The end of a gesture that held the altitude while it panned and zoomed
/// over a slope moves the center onto the slope along the view, which
/// leaves the eye where it was and the ground where it was on the screen,
/// and reports the camera's change.
#[test]
fn a_gesture_end_moves_neither_the_eye_nor_the_picture() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, _) = map_over(Some(&ground), &run_loop);
    map.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    map.apply_camera_delta(&delta(GesturePhase::Update, None, Some(1.5))).unwrap();
    map.apply_camera_delta(&delta(GesturePhase::Update, Some((-40.0, 10.0)), None)).unwrap();
    let held = map.transform_state().camera_options(None);
    assert_eq!(Some(0.0), held.center_altitude, "the gesture holds the altitude");
    let eye = map.transform_state().view().eye_position();
    let points = ground_points(&map);
    let drawn = pixels(map.transform_state().view(), &*ground, &points);
    map.drain_events();

    map.apply_camera_delta(&delta(GesturePhase::End, None, None)).unwrap();
    let events = map.drain_events();
    let ended = map.transform_state().camera_options(None);
    let altitude = ended.center_altitude.unwrap();
    assert!(altitude > 200.0, "the center moved onto the slope, to {altitude} m");
    assert!(ended.zoom.unwrap() > held.zoom.unwrap() + 0.01, "the zoom follows the nearer center");
    assert_eq!((held.pitch, held.bearing), (ended.pitch, ended.bearing));
    assert!(
        events.iter().any(
            |event| matches!(event, MapEvent::CameraDidChange(change) if change.source == CameraChangeSource::Gesture)
        ),
        "{events:?}"
    );
    assert_still_at(&mut map, &*ground, eye, 1e-3, &points, &drawn);
}

/// The end of a gesture's momentum, too, moves the center onto the ground
/// along the view, with the eye and the picture where its last step left
/// them.
#[test]
fn a_momentum_end_moves_neither_the_eye_nor_the_picture() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    map.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    // A glide of exactly a second, along a curve that ends at rest.
    let decay = MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: 1000.0,
            duration: Duration::from_secs(1),
            exponent: 0.5,
            easing: MomentumEasing::ANDROID_FLING,
        },
        ..MomentumDecay::DEFAULT
    };
    let glide = MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay };
    let momentum = CameraMomentum { pan: Some(glide), ..Default::default() };
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End, None, None) }).unwrap();
    assert!(map.transform_state().view().center_altitude() == 0.0 && map.snapshot().transition_in_progress);

    // A tenth of a millisecond before the end, the glide has all but stopped.
    map.set_frame_time(Some(clock + Duration::from_micros(999_900)));
    assert!(map.snapshot().transition_in_progress);
    let eye = map.transform_state().view().eye_position();
    let points = ground_points(&map);
    let drawn = pixels(map.transform_state().view(), &*ground, &points);
    map.set_frame_time(Some(clock + Duration::from_secs(1)));
    let snapshot = map.snapshot();
    assert!(!snapshot.transition_in_progress && !snapshot.gesture_in_progress);
    let altitude = map.transform_state().view().center_altitude();
    assert!(altitude.abs() > 500.0, "the center moved onto the slope, to {altitude} m");
    assert_still_at(&mut map, &*ground, eye, 1e-3, &points, &drawn);
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

/// A gesture's end, and the end of its momentum, move the center onto the
/// ground before the change that ends them reports it: a host that reads
/// the camera on that `CameraDidChange` reads the camera at rest, which
/// moves no more without another report.
#[test]
fn a_gesture_end_reports_the_recentred_camera_with_its_did_change() {
    let run_loop = Queue::<()>::new();
    let ground = slope();
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    let at_rest = |map: &mut Map, what: &str| {
        let view = map.transform_state().view().clone();
        let center = view.center(WrapMode::Wrapped);
        assert_eq!(ground.at(center).unwrap(), view.center_altitude(), "{what}: the center stands on the ground");
        map.trigger_repaint();
        map.set_frame_time(Some(clock + Duration::from_secs(5)));
        assert_eq!(view, *map.transform_state().view(), "{what}: the camera moves no more");
        assert_eq!(Vec::<(bool, CameraChangeSource)>::new(), camera_events(map), "{what}");
    };

    map.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    map.apply_camera_delta(&delta(GesturePhase::Update, None, Some(1.5))).unwrap();
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "the gesture holds the altitude");
    camera_events(&mut map);
    map.apply_camera_delta(&delta(GesturePhase::End, None, None)).unwrap();
    let altitude = map.transform_state().view().center_altitude();
    assert!(altitude > 10.0, "the end moved the center onto the slope, to {altitude} m");
    let gesture = CameraChangeSource::Gesture;
    assert_eq!(vec![(false, gesture), (true, gesture)], camera_events(&mut map), "the end");
    at_rest(&mut map, "after the end");

    // A glide of exactly a second.
    let (mut map, clock) = map_over(Some(&ground), &run_loop);
    map.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    let decay = MomentumDecay {
        model: MomentumDecayModel::Curve {
            reference_speed: 1000.0,
            duration: Duration::from_secs(1),
            exponent: 0.5,
            easing: MomentumEasing::ANDROID_FLING,
        },
        ..MomentumDecay::DEFAULT
    };
    let glide = MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay };
    let momentum = CameraMomentum { pan: Some(glide), ..Default::default() };
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End, None, None) }).unwrap();
    map.set_frame_time(Some(clock + Duration::from_millis(999)));
    assert_eq!(0.0, map.transform_state().view().center_altitude(), "the glide holds the altitude");
    camera_events(&mut map);
    map.set_frame_time(Some(clock + Duration::from_secs(1)));
    assert!(map.transform_state().view().center_altitude().abs() > 500.0, "the end moved the center onto the slope");
    assert_eq!(vec![(true, CameraChangeSource::Momentum)], camera_events(&mut map), "the glide's end");
    at_rest(&mut map, "after the glide");
}

/// A cone of ground 3000 m high, sloping 45 degrees, `north` meters north
/// of the start.
#[derive(Debug)]
pub(super) struct Hill {
    peak: DVec2,
}

impl Hill {
    const HEIGHT: f64 = 3000.0;

    pub(super) fn new(north: f64) -> Hill {
        Hill { peak: world(&start()) - DVec2::new(0.0, north / meters()) }
    }

    pub(super) fn height(&self, world: DVec2) -> f64 {
        (Self::HEIGHT - (world - self.peak).length() * meters()).max(0.0)
    }
}

/// Meters along the ground in a unit of Web Mercator at zoom 0, at the
/// start, on the sphere heights scale on.
pub(super) fn meters() -> f64 {
    Projection::altitude_meters_per_pixel(start().latitude(), 0.0) * 512.0
}

impl Elevation for Hill {
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
        Some((0.0, Self::HEIGHT))
    }
}

/// A map at rest at sea level at the start, at zoom 14 and pitch 60,
/// looking north at a hill 8 km away, in a gesture that moves its center
/// north 50 m at each step. Calls `each` with the view after each step.
fn over_the_hill(steps: u32, mut each: impl FnMut(&View, &Hill)) -> Map {
    let run_loop = Queue::<()>::new();
    let hill = Hill::new(8000.0);
    let (mut map, _) = map_over(None, &run_loop);
    map.jump_to(&CameraOptions { zoom: Some(14.0), pitch: Some(60.0), ..Default::default() });
    let drawn: Arc<dyn Elevation> = Arc::new(Hill { ..hill });
    report(&mut map, ground_event(&drawn));
    assert_eq!(0.0, ground(&map).0);
    for step in 0..steps {
        let center = world(&start()) - DVec2::new(0.0, f64::from(step) * 50.0 / meters());
        let center =
            Projection::unproject(&geometry::Point::new(center.x, center.y), 1.0 / 512.0, WrapMode::Unwrapped).unwrap();
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: CameraOptions { center: Some(center), ..Default::default() },
            phase: if step == 0 { GesturePhase::Begin } else { GesturePhase::Update },
            ..Default::default()
        });
        each(map.transform_state().view(), &hill);
    }
    map
}

/// Through a gesture into a hill, the eye and the near plane stay at or
/// above the ground at every step, lifted only as fast as the ground under
/// them rises: the altitude changes no faster than the 45-degree slope
/// under the moving eye and near plane allows (orbis's `Watch`).
#[test]
fn the_eye_is_kept_above_raised_ground_through_a_gesture() {
    let mut last: Option<(f64, [DVec3; 10])> = None;
    let mut highest = 0.0f64;
    over_the_hill(400, |view, hill| {
        let eye = view.eye_position();
        let mut points = [eye; 10];
        points[1..].copy_from_slice(&view.near_plane_points());
        for point in points {
            let ground = hill.height(point.truncate());
            assert!(point.z >= ground - 1e-6, "{point} under the ground at {ground} m");
        }
        let altitude = view.center_altitude();
        if let Some((before, was)) = last {
            let moved = points
                .iter()
                .zip(was)
                .map(|(point, was)| (point.truncate() - was.truncate()).length())
                .fold(0.0, f64::max);
            let rose = (altitude - before).abs();
            // The scale of heights changes a little with the latitude.
            assert!(rose <= moved * meters() + 0.01, "{before} m to {altitude} m over {} m", moved * meters());
        }
        highest = highest.max(altitude);
        last = Some((altitude, points));
    });
    assert!(highest > 1000.0, "the hill lifted the camera {highest} m");
}

/// Once the hill is past, the lift lowers again, to the altitude the
/// gesture holds.
#[test]
fn a_lift_lowers_once_the_hill_is_past() {
    let mut lifted = false;
    let map = over_the_hill(800, |view, _| lifted |= view.center_altitude() > 0.0);
    assert!(lifted);
    assert!(map.snapshot().gesture_in_progress);
    assert_eq!(0.0, map.transform_state().view().center_altitude());
}

/// A gesture's end whose move onto the ground the maximum zoom refuses
/// leaves the center where it is, and the ground under it takes the
/// center up, as at rest.
#[test]
fn a_refused_recentre_leaves_the_at_rest_rule() {
    for (max_zoom, recentred) in [(12.1, false), (13.0, true)] {
        let run_loop = Queue::<()>::new();
        let (mut map, _) = map_over(Some(&flat(Some(0.0), 0.0)), &run_loop);
        map.set_bounds(&BoundOptions { max_zoom: Some(max_zoom), ..Default::default() }).unwrap();
        map.apply_camera_delta(&delta(GesturePhase::Begin, Some((0.0, 10.0)), None)).unwrap();
        // Ground 1500 m high loads under the gesture, which holds sea level.
        report(&mut map, ground_event(&flat(Some(1500.0), 1500.0)));
        let held = map.transform_state().camera_options(None);
        assert_eq!(Some(0.0), held.center_altitude);
        map.apply_camera_delta(&delta(GesturePhase::End, None, None)).unwrap();
        let ended = map.transform_state().camera_options(None);
        assert_eq!(Some(1500.0), ended.center_altitude, "max zoom {max_zoom}");
        let center = |camera: &CameraOptions| camera.center.unwrap();
        if recentred {
            assert!(ended.zoom.unwrap() > 12.1, "max zoom {max_zoom}: {ended:?}");
            assert_ne!(center(&held), center(&ended));
        } else {
            assert_eq!((held.center, held.zoom), (ended.center, ended.zoom), "max zoom {max_zoom}");
        }
        let view = map.transform_state().view().clone();
        map.trigger_repaint();
        assert_eq!(view, *map.transform_state().view(), "max zoom {max_zoom}: the next update moves nothing");
    }
}

/// A gesture over a center the terrain doesn't clamp keeps the center's
/// altitude, and ground above the eye raises the eye by pitch and zoom,
/// as a command's. Its end leaves the center where it is.
#[test]
fn an_unclamped_gesture_keeps_its_altitude_and_is_raised_by_pitch_and_zoom() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(None, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions {
        zoom: Some(14.0),
        pitch: Some(60.0),
        center_altitude: Some(500.0),
        ..Default::default()
    });
    // Ground 3000 m high loads under the camera at rest, above its eye.
    report(&mut map, ground_event(&flat(Some(3000.0), 3000.0)));
    assert!(map.transform_state().view().eye_position().z < 3000.0);

    map.apply_camera_delta(&delta(GesturePhase::Begin, Some((0.0, 1.0)), None)).unwrap();
    let raised = map.transform_state().camera_options(None);
    assert_eq!(Some(500.0), raised.center_altitude);
    assert!(raised.pitch.unwrap() < 60.0 && raised.zoom.unwrap() < 14.0, "{raised:?}");
    assert!(map.transform_state().view().eye_position().z >= 3000.0 - 1e-6);

    map.apply_camera_delta(&delta(GesturePhase::End, None, None)).unwrap();
    let ended = map.transform_state().camera_options(None);
    assert_eq!((raised.center, raised.zoom, Some(500.0)), (ended.center, ended.zoom, ended.center_altitude));
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
        map.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
        let momentum = CameraMomentum {
            pan: Some(MomentumGlide { velocity: ScreenCoordinate::new(-600.0, 800.0), decay: decay(1, 1000.0) }),
            bearing: AnchoredMomentum {
                glide: turns.then_some(MomentumGlide { velocity: 30.0, decay: decay(3, 30.0) }),
                ..Default::default()
            },
            ..Default::default()
        };
        map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End, None, None) })
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

/// A momentum whose only part is a pitch snap that settles at once keeps
/// the eye and the near plane above the ground, as every other step does:
/// over a center the terrain doesn't clamp, the ground raises the snapped
/// eye by pitch and zoom.
#[test]
fn an_instant_snap_keeps_the_eye_above_the_ground() {
    let run_loop = Queue::<()>::new();
    let (mut map, _) = map_over(None, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions { zoom: Some(14.0), pitch: Some(50.0), ..Default::default() });
    let height = 1000.0;
    report(&mut map, ground_event(&flat(Some(height), height)));
    map.apply_camera_delta(&CameraDelta { pitch: Some(5.0), phase: GesturePhase::Begin, ..Default::default() })
        .unwrap();
    assert!(map.transform_state().view().eye_position().z > height + 500.0);
    // A snap from 55 to 80 degrees, which would put the eye under the ground.
    let snap = MomentumSnap { spacing: 80.0, offset: 0.0, tolerance: 30.0, duration: Duration::ZERO };
    let momentum =
        CameraMomentum { pitch: AnchoredMomentum { snap: Some(snap), ..Default::default() }, ..Default::default() };
    map.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End, None, None) }).unwrap();
    let snapshot = map.snapshot();
    assert!(!snapshot.gesture_in_progress && !snapshot.transition_in_progress);
    let view = map.transform_state().view();
    assert!(view.pitch().to_degrees() > 55.0, "the snap pitched the camera, to {}", view.pitch().to_degrees());
    assert!(view.eye_position().z >= height, "the eye at {} m", view.eye_position().z);
    for point in view.near_plane_points() {
        assert!(point.z >= height - 0.01, "{point} under the ground at {height} m");
    }
}

/// A momentum with nothing to move ends its gesture at once, and the end's
/// move onto the ground reports its change as every camera change does:
/// will change, the command's end, did change.
#[test]
fn an_empty_momentum_reports_its_recentre_in_order() {
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
    transform.apply_camera_delta(&delta(GesturePhase::Begin, Some((-60.0, 40.0)), None)).unwrap();
    let held = transform.view().center_altitude();
    transform.drain_events().for_each(drop);
    let momentum = CameraMomentum::default();
    transform
        .apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End, None, None) })
        .unwrap();
    assert!(!transform.is_gesture_in_progress() && transform.view().center_altitude() != held, "the end recentred");
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

/// A clamp turned off mid-gesture drops the altitude the gesture holds, and
/// one turned on again in the same gesture holds the camera's altitude then,
/// rather than the one the gesture began at.
#[test]
fn a_clamp_turned_on_again_mid_gesture_holds_the_altitude_then() {
    let mut lifted = 0.0;
    let mut map = over_the_hill(210, |view, _| lifted = view.center_altitude());
    assert!(lifted > 100.0, "the hill lifted the camera {lifted} m");
    map.set_center_clamped_to_ground(false);
    // Back off the hill, where the gesture's first altitude needs no lift.
    let center = world(&start()) + DVec2::new(0.0, 2000.0 / meters());
    let center =
        Projection::unproject(&geometry::Point::new(center.x, center.y), 1.0 / 512.0, WrapMode::Unwrapped).unwrap();
    let step = |map: &mut Map| {
        map.update_camera(&CameraUpdate {
            mode: CameraUpdateMode::Jump,
            camera: CameraOptions { center: Some(center), ..Default::default() },
            phase: GesturePhase::Update,
            ..Default::default()
        });
    };
    step(&mut map);
    assert_eq!(lifted, map.transform_state().view().center_altitude(), "unclamped, the altitude stays");
    map.set_center_clamped_to_ground(true);
    step(&mut map);
    assert_eq!(lifted, map.transform_state().view().center_altitude(), "the gesture holds the altitude it has");
}
