// The C++ AnimationOptions callbacks capture the Transform by reference and
// assert on it. Here the transform reports each command's end as an event,
// which the tests find by the command's transition id (`EndLog`) right after
// the call that ended it. The checks of the C++ frame callbacks run after
// each `update_transitions` that leaves the command running.
//
// Commands start at the frame time that `start_clock` sets, so a test steps
// them from a start it knows.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use glam::{DMat4, DQuat, DVec3, DVec4, EulerRot, dvec3};
use mln_map_types::{
    AxonometricMode, CameraDelta, CameraError, CameraMomentum, CameraUpdate, CameraUpdateMode, CameraVelocity,
    GesturePhase, MomentumDecay, MomentumDecayModel, MomentumEasing, MomentumGlide, MomentumSnap, TileProjector,
    TransitionOutcome,
};
use mln_style::constants::{EXTENT, LATITUDE_MAX, LONGITUDE_MAX, TILE_SIZE_D};
use mln_style::geo::LatLngBounds;
use mln_style::math::deg2rad;
use mln_style::tile_id::CanonicalTileID;
use mln_style::time::Instant;
use mln_style::types::ProjectionType;
use mln_style::unitbezier::UnitBezier;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::MAX_ZOOM;

use super::momentum::GlidePlan;
use super::*;
use crate::map_event::CameraChangeMode;

fn new_transform() -> Transform {
    Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default)
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

fn sc(x: f64, y: f64) -> ScreenCoordinate {
    ScreenCoordinate::new(x, y)
}

fn insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    EdgeInsets::new(top, left, bottom, right)
}

/// gtest's `FloatingPoint<T>::AlmostEquals` distance: sign-and-magnitude
/// bits mapped to biased integers.
fn biased_f64(x: f64) -> u64 {
    const SIGN: u64 = 1 << 63;
    let bits = x.to_bits();
    if bits & SIGN != 0 { (!bits).wrapping_add(1) } else { bits | SIGN }
}

fn biased_f32(x: f32) -> u32 {
    const SIGN: u32 = 1 << 31;
    let bits = x.to_bits();
    if bits & SIGN != 0 { (!bits).wrapping_add(1) } else { bits | SIGN }
}

/// `actual` at C++'s screen point `expected`, within the 1e-7 of its
/// distance from the origin that C++'s float field of view moves it.
#[track_caller]
fn assert_cpp_pixel(expected: f64, actual: f64) {
    assert_near(expected, actual, expected.abs() * 1e-7 + 1e-9);
}

/// `EXPECT_DOUBLE_EQ`/`ASSERT_DOUBLE_EQ`: at most 4 ULPs apart, NaN never
/// equal.
#[track_caller]
fn assert_double_eq(val1: f64, val2: f64) {
    let equal = !val1.is_nan() && !val2.is_nan() && biased_f64(val1).abs_diff(biased_f64(val2)) <= 4;
    assert!(equal, "DOUBLE_EQ: {val1:?} vs {val2:?}");
}

/// `EXPECT_FLOAT_EQ`: both values converted to float, at most 4 float ULPs
/// apart.
#[track_caller]
fn assert_float_eq(val1: f64, val2: f64) {
    let (a, b) = (val1 as f32, val2 as f32);
    let equal = !a.is_nan() && !b.is_nan() && biased_f32(a).abs_diff(biased_f32(b)) <= 4;
    assert!(equal, "FLOAT_EQ: {a:?} vs {b:?}");
}

/// `EXPECT_NEAR`/`ASSERT_NEAR`.
#[track_caller]
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    let diff = (val1 - val2).abs();
    assert!(diff <= abs_error, "NEAR: |{val1:?} - {val2:?}| = {diff:?} > {abs_error:?}");
}

const ABS_DOUBLE_ERROR: f64 = 1e-5;

#[track_caller]
fn assert_vec3_near_1e5(arg: DVec3, vec: DVec3) {
    assert!(arg.abs_diff_eq(vec, ABS_DOUBLE_ERROR), "{arg:?} vs {vec:?}");
}

/// The C++ tests' setters of one limit each, which leave the limits as they
/// were where C++ ignores the value.
trait LimitSetters {
    fn limits(&self) -> CameraLimits;
    fn set_one_limit(&mut self, limits: CameraLimits) {
        let _ = self.set_limits_to(limits);
    }
    fn set_limits_to(&mut self, limits: CameraLimits) -> Result<(), Error>;
    fn set_lat_lng_bounds(&mut self, bounds: LatLngBounds) -> Result<(), Error> {
        self.set_limits_to(CameraLimits { bounds, ..self.limits() })
    }
    fn set_min_zoom(&mut self, min_zoom: f64) {
        self.set_one_limit(CameraLimits { min_zoom, ..self.limits() });
    }
    fn set_max_zoom(&mut self, max_zoom: f64) {
        self.set_one_limit(CameraLimits { max_zoom, ..self.limits() });
    }
    /// In degrees.
    fn set_min_pitch(&mut self, min_pitch: f64) {
        self.set_one_limit(CameraLimits { min_pitch: deg2rad(min_pitch), ..self.limits() });
    }
    /// In degrees.
    fn set_max_pitch(&mut self, max_pitch: f64) {
        self.set_one_limit(CameraLimits { max_pitch: deg2rad(max_pitch), ..self.limits() });
    }
}

impl LimitSetters for Transform {
    fn limits(&self) -> CameraLimits {
        *self.state().limits()
    }
    fn set_limits_to(&mut self, limits: CameraLimits) -> Result<(), Error> {
        self.set_limits(limits)
    }
}

/// Formats `value` with six decimals, as `std::to_string(double)` does.
fn std_to_string(value: f64) -> String {
    format!("{value:.6}")
}

/// gtest's `SCOPED_TRACE`: prints the trace when a failed assertion unwinds
/// the scope.
struct ScopedTrace(String);

impl Drop for ScopedTrace {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("trace: {}", self.0);
        }
    }
}

fn hypot3(x: f64, y: f64, z: f64) -> f64 {
    (x * x + y * y + z * z).sqrt()
}

/// Clamps `value` to `lo..=hi` as `std::clamp` does:
/// `value < lo ? lo : hi < value ? hi : value`.
fn std_clamp(value: f64, lo: f64, hi: f64) -> f64 {
    if value < lo {
        lo
    } else if hi < value {
        hi
    } else {
        value
    }
}

fn rotated_frame(q: DQuat) -> (DVec3, DVec3, DVec3) {
    (q * DVec3::X, q * DVec3::NEG_Y, q * DVec3::NEG_Z)
}

/// The rotation by `x` about x, then `y` about y, then `z` about z.
fn euler(x: f64, y: f64, z: f64) -> DQuat {
    DQuat::from_euler(EulerRot::ZYX, z, y, x)
}

/// Starts the commands made after it at the returned time: a frame time an
/// hour ahead of the clock, which stays fresh.
fn start_clock(transform: &mut Transform) -> Instant {
    let start = Instant::now() + Duration::from_secs(3600);
    transform.set_frame_time(Some(start));
    start
}

/// Finds the ends of the commands with one transition id among what the
/// transform reported (see the file comment).
struct EndLog {
    transition_id: u64,
    /// The ends that [`Self::take_finish`] has seen.
    seen: Cell<usize>,
}

impl EndLog {
    /// Gives `options` a transition id of its own, unless it has one,
    /// whose commands' ends the log finds.
    fn install(options: &mut AnimationOptions) -> EndLog {
        static NEXT: AtomicU64 = AtomicU64::new(1 << 32);
        let transition_id = *options.transition_id.get_or_insert_with(|| NEXT.fetch_add(1, Ordering::Relaxed));
        EndLog { transition_id, seen: Cell::new(0) }
    }

    /// The ends of the log's commands, oldest first.
    fn ends(&self, transform: &Transform) -> Vec<TransitionEnd> {
        transform
            .events
            .iter()
            .filter_map(|event| match event {
                TransformEvent::CommandEnded(end) if end.transition_id == Some(self.transition_id) => Some(end.end),
                _ => None,
            })
            .collect()
    }

    /// How many of the log's commands ended.
    fn count(&self, transform: &Transform) -> usize {
        self.ends(transform).len()
    }

    /// How the log's one command ended, if it has.
    fn get(&self, transform: &Transform) -> Option<TransitionEnd> {
        let ends = self.ends(transform);
        assert!(ends.len() <= 1, "a command ends once");
        ends.first().copied()
    }

    /// Whether a command of the log ended since the last call.
    fn take_finish(&self, transform: &Transform) -> bool {
        let count = self.count(transform);
        count > self.seen.replace(count)
    }
}

/// Takes the will-change ('w') and did-change ('d') events that the
/// transform reported, with their changes, and leaves the others.
fn take_changes(transform: &mut Transform) -> Vec<(char, CameraChange)> {
    transform
        .events
        .extract_if(.., |event| {
            matches!(event, TransformEvent::CameraWillChange(_) | TransformEvent::CameraDidChange(_))
        })
        .map(|event| match event {
            TransformEvent::CameraWillChange(change) => ('w', change),
            TransformEvent::CameraDidChange(change) => ('d', change),
            _ => unreachable!(),
        })
        .collect()
}

// gtest: Transform.InvalidZoom
#[test]
fn invalid_zoom() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.view().zoom());

    transform.jump_to(&CameraOptions { zoom: Some(1.0), ..Default::default() });

    assert_double_eq(1.0, transform.view().zoom());

    let invalid = f64::NAN;

    transform.jump_to(&CameraOptions { zoom: Some(invalid), ..Default::default() });

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.view().zoom());

    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(invalid), ..Default::default() });

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.view().zoom());

    let max_zoom = transform.state().limits().max_zoom;
    transform.jump_to(&CameraOptions { zoom: Some(max_zoom + 0.1), ..Default::default() });
    assert_double_eq(transform.view().zoom(), transform.state().limits().max_zoom);

    // Executing flyTo with an empty size causes frameZoom to be NaN.
    let max_zoom = transform.state().limits().max_zoom;
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(LATITUDE_MAX, LONGITUDE_MAX)), zoom: Some(max_zoom), ..Default::default() },
        &AnimationOptions::default(),
        false,
    );
    run_to_end(&mut transform, start);
    assert_double_eq(transform.view().zoom(), transform.state().limits().max_zoom);

    // Executing flyTo with maximum zoom level to the same zoom level causes
    // frameZoom to be bigger than maximum zoom.
    transform.resize(Size::new(100, 100)).unwrap();
    let max_zoom = transform.state().limits().max_zoom;
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(LATITUDE_MAX, LONGITUDE_MAX)), zoom: Some(max_zoom), ..Default::default() },
        &AnimationOptions::default(),
        false,
    );
    run_to_end(&mut transform, start);
    assert!(!transform.in_transition());

    assert!(transform.state().is_valid());
    assert_double_eq(transform.state().limits().max_zoom, transform.view().zoom());
}

// gtest: Transform.InvalidBearing
#[test]
fn invalid_bearing() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.view().zoom());

    transform.jump_to(&CameraOptions { zoom: Some(1.0), bearing: Some(2.0), ..Default::default() });
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.view().zoom());
    assert_double_eq(deg2rad(-2.0), transform.view().angle());

    let invalid = f64::NAN;

    transform.jump_to(&CameraOptions { bearing: Some(invalid), ..Default::default() });
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.view().zoom());
    assert_double_eq(deg2rad(-2.0), transform.view().angle());
}

// gtest: Transform.IntegerZoom
#[test]
fn integer_zoom() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    let mut check_integer_zoom = |zoom_int: u8, zoom: f64| {
        transform.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        assert_near(transform.view().zoom(), zoom, 1e-8);
        assert_eq!(transform.state().view().integer_zoom(), zoom_int);
        assert_near(transform.state().view().zoom_fraction(), zoom - f64::from(zoom_int), 1e-8);
    };

    for zoom_int in 0u8..20 {
        for percent in 0u32..100 {
            let zoom = f64::from(zoom_int) + (0.01 * f64::from(percent));
            check_integer_zoom(zoom_int, zoom);
        }
    }

    // Special case zoom 20.
    check_integer_zoom(20, 20.0);
}

// gtest: Transform.PerspectiveProjection
#[test]
fn perspective_projection() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    // 0.9 rad ~ 51.56620156 deg
    transform.jump_to(&CameraOptions {
        center: Some(ll(38.0, -77.0)),
        zoom: Some(10.0),
        pitch: Some(51.566_201_56),
        ..Default::default()
    });

    // expected values are from maplibre-gl-js

    let loc = transform.view().center(WrapMode::Wrapped);
    assert_double_eq(-77.0, loc.longitude());
    assert_double_eq(38.0, loc.latitude());

    let loc = transform.state().view().screen_coordinate_to_lat_lng(&sc(0.0, 1000.0), WrapMode::Unwrapped).unwrap();
    assert_near(-77.591_989_611_991_48, loc.longitude(), 1e-6);
    assert_near(38.746_613_263_020_18, loc.latitude(), 1e-6);

    let loc = transform.state().view().screen_coordinate_to_lat_lng(&sc(1000.0, 0.0), WrapMode::Unwrapped).unwrap();
    assert_near(-76.758_232_392_056_41, loc.longitude(), 1e-6);
    assert_near(37.692_872_969_426_375, loc.latitude(), 1e-6);

    let point =
        transform.state().view().lat_lng_to_screen_coordinate(&ll(38.746_613_263_020_18, -77.591_989_611_991_48));
    assert_near(point.x, 0.0, 1e-5);
    assert_near(point.y, 1000.0, 1e-4);

    let point =
        transform.state().view().lat_lng_to_screen_coordinate(&ll(37.692_872_969_426_375, -76.758_232_392_056_41));
    assert_near(point.x, 1000.0, 1e-5);
    assert_near(point.y, 0.0, 1e-4);

    let (point, p) = transform
        .state()
        .view()
        .lat_lng_to_screen_coordinate_with_clip(&ll(37.692_872_969_426_375, -76.758_232_392_056_41));
    assert_near(point.x, 1000.0, 1e-5);
    assert_near(point.y, 0.0, 1e-4);
    assert!(p[3] > 0.0);

    transform.jump_to(&CameraOptions {
        center: Some(ll(38.0, -77.0)),
        zoom: Some(18.0),
        pitch: Some(51.566_201_56),
        ..Default::default()
    });
    let (_, p) = transform
        .state()
        .view()
        .lat_lng_to_screen_coordinate_with_clip(&ll(7.692_872_969_426_375, -76.758_232_392_056_41));
    assert!(p[3] < 0.0);
}

// gtest: Transform.UnwrappedLatLng
#[test]
fn unwrapped_lat_lng() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    // 0.9 rad ~ 51.56620156 deg
    transform.jump_to(&CameraOptions {
        center: Some(ll(38.0, -77.0)),
        zoom: Some(10.0),
        pitch: Some(51.566_201_56),
        ..Default::default()
    });

    let state = transform.state();

    let from_get_lat_lng = state.view().center(WrapMode::Unwrapped);
    assert_double_eq(from_get_lat_lng.latitude(), 38.0);
    assert_double_eq(from_get_lat_lng.longitude(), -77.0);

    let from_screen_coordinate =
        state.view().screen_coordinate_to_lat_lng(&sc(500.0, 500.0), WrapMode::Unwrapped).unwrap();
    assert_near(from_screen_coordinate.latitude(), 38.0, 1e-8);
    assert_near(from_screen_coordinate.longitude(), -77.0, 1e-8);

    let mut wrapped_rightwards = state
        .view()
        .screen_coordinate_to_lat_lng(&state.view().lat_lng_to_screen_coordinate(&ll(38.0, 283.0)), WrapMode::Unwrapped)
        .unwrap();
    assert_near(wrapped_rightwards.latitude(), 38.0, 1e-8);
    assert_near(wrapped_rightwards.longitude(), 283.0, 1e-8);
    wrapped_rightwards.wrap();
    assert_near(wrapped_rightwards.longitude(), -77.0, 1e-8);

    let mut wrapped_leftwards = state
        .view()
        .screen_coordinate_to_lat_lng(
            &state.view().lat_lng_to_screen_coordinate(&ll(38.0, -437.0)),
            WrapMode::Unwrapped,
        )
        .unwrap();
    assert_double_eq(wrapped_leftwards.latitude(), wrapped_rightwards.latitude());
    assert_near(wrapped_leftwards.longitude(), -437.0, 1e-8);
    wrapped_leftwards.wrap();
    assert_near(wrapped_leftwards.longitude(), -77.0, 1e-8);
}

// The copy unwrapForShortestPath finds for a wrapped center, a point 180
// degrees away included, and that copy moved into the center's own world when
// bounds keep the center past the antimeridian.
// gtest: Transform.LatLngNearCenter
#[test]
fn lat_lng_near_center() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -10.0)), zoom: Some(4.0), ..Default::default() });
    assert_near(-181.0, transform.state().view().lat_lng_near_center(&ll(0.0, 179.0)).longitude(), 1e-9);
    assert_near(170.0, transform.state().view().lat_lng_near_center(&ll(0.0, 170.0)).longitude(), 1e-9);

    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 181.0)), ..Default::default() });
    assert_near(181.0, transform.state().view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(179.0, transform.state().view().lat_lng_near_center(&ll(0.0, 179.0)).longitude(), 1e-9);
    assert_near(180.2, transform.state().view().lat_lng_near_center(&ll(0.0, -179.8)).longitude(), 1e-9);
    assert_near(350.0, transform.state().view().lat_lng_near_center(&ll(0.0, -10.0)).longitude(), 1e-9);
}

// gtest: Transform.ConstrainHeightOnly
#[test]
fn constrain_height_only() {
    let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(2, 2)).unwrap();

    transform.jump_to(&CameraOptions {
        center: Some(LatLngBounds::world().southwest()),
        zoom: Some(MAX_ZOOM),
        ..Default::default()
    });
    assert_near(-LATITUDE_MAX, transform.view().center(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.view().center(WrapMode::Wrapped).longitude(), 1e-7);

    transform.jump_to(&CameraOptions { center: Some(LatLngBounds::world().northeast()), ..Default::default() });
    assert_near(LATITUDE_MAX, transform.view().center(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.view().center(WrapMode::Wrapped).longitude(), 1e-7);
}

// gtest: Transform.ConstrainWidthAndHeight
#[test]
fn constrain_width_and_height() {
    let mut transform = Transform::new(ConstrainMode::WidthAndHeight, ViewportMode::Default);
    transform.resize(Size::new(2, 2)).unwrap();

    transform.jump_to(&CameraOptions {
        center: Some(LatLngBounds::world().southwest()),
        zoom: Some(MAX_ZOOM),
        ..Default::default()
    });
    assert_near(-LATITUDE_MAX, transform.view().center(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);

    transform.jump_to(&CameraOptions { center: Some(LatLngBounds::world().northeast()), ..Default::default() });
    assert_near(LATITUDE_MAX, transform.view().center(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
}

// gtest: Transform.Anchor
#[test]
fn anchor() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    let lat_lng = ll(10.0, -100.0);
    let anchor_point = sc(150.0, 150.0);

    transform.jump_to(&CameraOptions { center: Some(lat_lng), zoom: Some(10.0), ..Default::default() });
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(10.0, transform.view().zoom());
    assert_double_eq(0.0, transform.view().angle());

    let anchor_lat_lng =
        transform.state().view().screen_coordinate_to_lat_lng(&anchor_point, WrapMode::Unwrapped).unwrap();
    assert_ne!(lat_lng.latitude(), anchor_lat_lng.latitude());
    assert_ne!(lat_lng.longitude(), anchor_lat_lng.longitude());

    transform.jump_to(&CameraOptions { center: Some(lat_lng), zoom: Some(3.0), ..Default::default() });
    assert_double_eq(3.0, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(3.5), ..Default::default() });
    assert_double_eq(3.5, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(5.5), anchor: Some(anchor_point), ..Default::default() });
    assert_double_eq(5.5, transform.view().zoom());
    assert_ne!(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { center: Some(lat_lng), zoom: Some(3.0), ..Default::default() });
    assert_double_eq(3.0, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(5.0), ..Default::default() });
    assert_double_eq(5.0, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(7.0), anchor: Some(anchor_point), ..Default::default() });
    assert_double_eq(7.0, transform.view().zoom());
    assert_ne!(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { center: Some(lat_lng), zoom: Some(2.0), ..Default::default() });
    assert_double_eq(2.0, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(4.0), ..Default::default() });
    assert_double_eq(4.0, transform.view().zoom());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { zoom: Some(8.0), anchor: Some(anchor_point), ..Default::default() });
    assert_double_eq(8.0, transform.view().zoom());
    assert_ne!(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions {
        center: Some(lat_lng),
        zoom: Some(10.0),
        bearing: Some(-45.0),
        ..Default::default()
    });
    assert_double_eq(PI / 4.0, transform.view().angle());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { bearing: Some(0.0), ..Default::default() });
    assert_double_eq(0.0, transform.view().angle());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { bearing: Some(45.0), anchor: Some(anchor_point), ..Default::default() });
    assert_double_eq(deg2rad(-45.0), transform.view().angle());

    // Anchor coordinates are imprecise because we are converting from an integer pixel.
    assert_near(anchor_lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude(), 0.5);
    assert_near(anchor_lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude(), 0.5);

    transform.jump_to(&CameraOptions {
        center: Some(lat_lng),
        zoom: Some(10.0),
        pitch: Some(10.0),
        ..Default::default()
    });
    assert_double_eq(deg2rad(10.0), transform.view().pitch());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { pitch: Some(15.0), ..Default::default() });
    assert_double_eq(deg2rad(15.0), transform.view().pitch());
    assert_double_eq(lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude());

    transform.jump_to(&CameraOptions { pitch: Some(20.0), anchor: Some(anchor_point), ..Default::default() });
    assert_double_eq(deg2rad(20.0), transform.view().pitch());

    // Anchor coordinates are imprecise because we are converting from an integer pixel.
    assert_near(anchor_lat_lng.latitude(), transform.view().center(WrapMode::Wrapped).latitude(), 0.5);
    assert_near(anchor_lat_lng.longitude(), transform.view().center(WrapMode::Wrapped).longitude(), 0.5);
}

// gtest: Transform.Padding
#[test]
fn padding() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    let non_padded_camera_options =
        CameraOptions { center: Some(ll(10.0, -100.0)), zoom: Some(10.0), ..Default::default() };
    transform.jump_to(&non_padded_camera_options);

    let true_center = transform.view().center(WrapMode::Wrapped);
    assert_double_eq(10.0, true_center.latitude());
    assert_double_eq(-100.0, true_center.longitude());
    assert_double_eq(10.0, transform.view().zoom());

    let screen_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 / 2.0), WrapMode::Wrapped).unwrap();
    let upper_half_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 * 0.25), WrapMode::Wrapped).unwrap();

    let padding = insets(1000.0 / 2.0, 0.0, 0.0, 0.0);
    // CameraOption center and zoom don't change when padding changes: center of
    // viewport remains the same as padding defines viwport center offset in rendering.
    let padded_options = CameraOptions { padding: Some(padding), ..Default::default() };
    transform.jump_to(&padded_options);
    let the_same_center = transform.view().center(WrapMode::Wrapped);
    assert_double_eq(true_center.latitude(), the_same_center.latitude());
    assert_double_eq(true_center.longitude(), the_same_center.longitude());

    // However, LatLng is now at the center of lower half - verify conversion
    // from screen coordinate to LatLng.
    let padded_lower_half_screen_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 * 0.75), WrapMode::Wrapped).unwrap();
    assert_near(screen_center.latitude(), padded_lower_half_screen_center.latitude(), 1e-10);
    assert_near(screen_center.longitude(), padded_lower_half_screen_center.longitude(), 1e-10);

    // LatLng previously in upper half center, should now be under screen center.
    let padded_screen_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 / 2.0), WrapMode::Wrapped).unwrap();
    assert_near(upper_half_center.latitude(), padded_screen_center.latitude(), 1e-10);
    assert_near(upper_half_center.longitude(), padded_screen_center.longitude(), 1e-10);
}

// gtest: Transform.MoveBy
#[test]
fn move_by() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(10.0), ..Default::default() });

    let mut true_center = transform.view().center(WrapMode::Wrapped);
    assert_double_eq(0.0, true_center.latitude());
    assert_double_eq(0.0, true_center.longitude());
    assert_double_eq(10.0, transform.view().zoom());

    for x in 0u8..20 {
        let odd = x % 2 != 0;
        let forward = x % 10 != 0;

        let coordinate = transform
            .screen_coordinate_to_lat_lng(
                &sc(if odd { 400.0 } else { 600.0 }, if forward { 400.0 } else { 600.0 }),
                WrapMode::Wrapped,
            )
            .unwrap();
        transform
            .move_by(
                &sc(if odd { 100.0 } else { -100.0 }, if forward { 100.0 } else { -100.0 }),
                &AnimationOptions::default(),
            )
            .unwrap();

        true_center = transform.view().center(WrapMode::Wrapped);
        assert_near(coordinate.latitude(), true_center.latitude(), 1e-8);
        assert_near(coordinate.longitude(), true_center.longitude(), 1e-8);
    }

    // We have ~1.1 precision loss for each coordinate for 20 rounds of moveBy.
    assert_near(0.0, true_center.latitude(), 1.1);
    assert_near(0.0, true_center.longitude(), 1.1);
}

// On a pitched map a large (fling-sized) pan offset must be reduced so it can
// never reach the horizon -- otherwise the unprojected pan direction flips and the
// pan reverses (or lands in the tile-less void). See maplibre-native#3105.
// gtest: Transform.MoveByPitchedDoesNotCrossHorizon
#[test]
fn move_by_pitched_does_not_cross_horizon() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let start = CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(10.0), pitch: Some(60.0), ..Default::default() };

    // Panning "up" the screen on a north-up pitched map moves the center north.
    transform.jump_to(&start);
    transform.move_by(&sc(0.0, 300.0), &AnimationOptions::default()).unwrap();
    let moderate_lat = transform.view().center(WrapMode::Wrapped).latitude();

    // A fling-sized offset toward the horizon must keep moving the center the same
    // way (north) -- before the fix it reversed south and stay bounded (clamped
    // short of the horizon).
    transform.jump_to(&start);
    transform.move_by(&sc(0.0, 100_000.0), &AnimationOptions::default()).unwrap();
    let fling_lat = transform.view().center(WrapMode::Wrapped).latitude();

    assert!(moderate_lat > 0.0, "{moderate_lat}"); // moved north
    assert!(fling_lat > 0.0, "{fling_lat}"); // still north; not reversed
    assert!(fling_lat >= moderate_lat, "{fling_lat} {moderate_lat}"); // fling goes at least as far as the moderate pan
    assert!(fling_lat < LATITUDE_MAX, "{fling_lat}"); // but is bounded, not off near the horizon
}

// gtest: Transform.Antimeridian
#[test]
fn antimeridian() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(1.0), ..Default::default() });

    // San Francisco
    let coordinate_san_francisco = ll(37.7833, -122.4167);
    let pixel_sf = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_cpp_pixel(151.792_494_371_764_32, pixel_sf.x);
    assert_cpp_pixel(383.767_207_825_276_6, pixel_sf.y);

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -181.0)), ..Default::default() });

    let pixel_sf_longest = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_cpp_pixel(-357.363_066_164_128_16, pixel_sf_longest.x);
    assert_double_eq(pixel_sf.y, pixel_sf_longest.y);
    let mut unwrapped_sf = coordinate_san_francisco.wrapped();
    unwrapped_sf.unwrap_for_shortest_path(&transform.view().center(WrapMode::Wrapped));

    let pixel_sf_shortest = transform.lat_lng_to_screen_coordinate(&unwrapped_sf);
    assert_cpp_pixel(666.636_943_852_191_7, pixel_sf_shortest.x);
    assert_double_eq(pixel_sf.y, pixel_sf_shortest.y);

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), ..Default::default() });
    let pixel_sf_shortest = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_double_eq(pixel_sf_longest.x, pixel_sf_shortest.x);
    assert_double_eq(pixel_sf_longest.y, pixel_sf_shortest.y);

    // Waikiri
    let coordinate_waikiri = ll(-16.9310, 179.9787);
    transform.jump_to(&CameraOptions { center: Some(coordinate_waikiri), zoom: Some(10.0), ..Default::default() });
    let pixel_waikiri = transform.lat_lng_to_screen_coordinate(&coordinate_waikiri);
    assert_cpp_pixel(500.0, pixel_waikiri.x);
    assert_cpp_pixel(500.0, pixel_waikiri.y);

    transform
        .jump_to(&CameraOptions { center: Some(ll(coordinate_waikiri.latitude(), 180.0213)), ..Default::default() });
    let pixel_waikiri_longest = transform.lat_lng_to_screen_coordinate(&coordinate_waikiri);
    assert_cpp_pixel(524_725.964_381_082_3, pixel_waikiri_longest.x);
    assert_double_eq(pixel_waikiri.y, pixel_waikiri_longest.y);

    let mut unwrapped_waikiri = coordinate_waikiri.wrapped();
    unwrapped_waikiri.unwrap_for_shortest_path(&transform.view().center(WrapMode::Wrapped));
    let pixel_waikiri_shortest = transform.lat_lng_to_screen_coordinate(&unwrapped_waikiri);
    assert_cpp_pixel(437.959_252_726_483_44, pixel_waikiri_shortest.x);
    assert_double_eq(pixel_waikiri.y, pixel_waikiri_shortest.y);

    let coordinate_from_pixel =
        transform.screen_coordinate_to_lat_lng(&pixel_waikiri_longest, WrapMode::Wrapped).unwrap();
    assert_near(coordinate_waikiri.latitude(), coordinate_from_pixel.latitude(), 1e-4);
    assert_near(coordinate_waikiri.longitude(), coordinate_from_pixel.longitude(), 1e-4);

    transform
        .jump_to(&CameraOptions { center: Some(ll(coordinate_waikiri.latitude(), 180.0213)), ..Default::default() });
    let pixel_waikiri_shortest = transform.lat_lng_to_screen_coordinate(&coordinate_waikiri);
    assert_double_eq(pixel_waikiri_longest.x, pixel_waikiri_shortest.x);
    assert_double_eq(pixel_waikiri_longest.y, pixel_waikiri_shortest.y);

    let coordinate_from_pixel =
        transform.screen_coordinate_to_lat_lng(&pixel_waikiri_shortest, WrapMode::Wrapped).unwrap();
    assert_near(coordinate_waikiri.latitude(), coordinate_from_pixel.latitude(), 1e-4);
    assert_near(coordinate_waikiri.longitude(), coordinate_from_pixel.longitude(), 1e-4);
}

// A zoom around a point across the antimeridian leaves the center wrapped, as GL JS's setLocationAtPoint does. Bounds
// across the antimeridian keep it in their range, as easeTo does.
// gtest: Transform.ZoomAroundAPointAcrossTheAntimeridian
#[test]
fn zoom_around_a_point_across_the_antimeridian() {
    for bounded in [false, true] {
        let trace = if bounded { "bounds from 170 to 190 degrees" } else { "no bounds" };
        let mut transform = new_transform();
        transform.resize(Size::new(256, 256)).unwrap();
        if bounded {
            transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))).unwrap();
        }
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 179.9)), zoom: Some(5.0), ..Default::default() });
        transform.jump_to(&CameraOptions { zoom: Some(6.0), anchor: Some(sc(230.0, 128.0)), ..Default::default() });

        let longitude = transform.view().center(WrapMode::Unwrapped).longitude();
        if bounded {
            assert!(longitude > 180.0, "{trace}: {longitude}");
            assert!(longitude < 190.0, "{trace}: {longitude}");
        } else {
            assert!(longitude > -180.0, "{trace}: {longitude}");
            assert!(longitude < -178.0, "{trace}: {longitude}");
        }
    }
}

// gtest: Transform.Camera
#[test]
fn camera() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    let lat_lng1 = ll(45.0, 135.0);
    let camera_options1 = CameraOptions { center: Some(lat_lng1), zoom: Some(20.0), ..Default::default() };
    transform.jump_to(&camera_options1);
    assert_double_eq(lat_lng1.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng1.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(20.0, transform.view().zoom());

    let lat_lng2 = ll(-45.0, -135.0);
    let camera_options2 = CameraOptions { center: Some(lat_lng2), zoom: Some(10.0), ..Default::default() };
    transform.jump_to(&camera_options2);
    assert_double_eq(lat_lng2.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng2.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(10.0, transform.view().zoom());

    let mut ease_options = AnimationOptions::new(Duration::from_secs(1));
    let ease_log = EndLog::install(&mut ease_options);
    let check_ease = |transform: &Transform| {
        if transform.in_transition() {
            assert!(lat_lng1.latitude() >= transform.view().center(WrapMode::Wrapped).latitude());
            assert!(lat_lng1.longitude() <= transform.view().center(WrapMode::Wrapped).longitude());
        }
        if ease_log.take_finish(transform) {
            assert_double_eq(lat_lng1.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
            assert_double_eq(lat_lng1.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
            assert_double_eq(20.0, transform.view().zoom());
        }
    };

    let start = start_clock(&mut transform);
    transform.ease_to(&camera_options1, &ease_options);
    assert!(transform.in_transition());
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    check_ease(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    check_ease(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    check_ease(&transform);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    check_ease(&transform);
    assert!(!transform.in_transition());

    let mut fly_options = AnimationOptions::new(Duration::from_secs(1));
    let fly_log = EndLog::install(&mut fly_options);
    let check_fly = |transform: &Transform| {
        if transform.in_transition() {
            assert!(lat_lng2.latitude() <= transform.view().center(WrapMode::Wrapped).latitude());
            assert!(lat_lng2.longitude() >= transform.view().center(WrapMode::Unwrapped).longitude());
        }
        if fly_log.take_finish(transform) {
            // XXX Fix precision loss in flyTo:
            // https://github.com/mapbox/mapbox-gl-native/issues/4298
            assert_double_eq(lat_lng2.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
            assert_double_eq(lat_lng2.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
            assert_near(10.0, transform.view().zoom(), 1e-5);
        }
    };

    let start = start_clock(&mut transform);
    transform.fly_to(&camera_options2, &fly_options, false);
    assert!(transform.in_transition());
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    check_fly(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    check_fly(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    check_fly(&transform);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    check_fly(&transform);
    assert!(!transform.in_transition());

    // Anchor and center points are mutually exclusive.
    let camera = CameraOptions {
        center: Some(ll(0.0, 0.0)),
        anchor: Some(sc(0.0, 0.0)), // top-left
        zoom: Some(transform.state().limits().max_zoom),
        ..CameraOptions::default()
    };
    let start = start_clock(&mut transform);
    transform.ease_to(&camera, &AnimationOptions::new(Duration::from_secs(1)));
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_double_eq(transform.view().center(WrapMode::Wrapped).latitude(), 0.0);
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 0.0);
}

// gtest: Transform.ProjectionMode
#[test]
fn projection_mode() {
    let mut transform = new_transform();

    transform.set_projection_mode(&ProjectionMode::default().with_axonometric(true).with_x_skew(1.0).with_y_skew(0.0));
    let options = transform.view().projection_mode();

    assert_eq!(options.axonometric.unwrap(), AxonometricMode::Enabled);
    assert_eq!(options.x_skew.unwrap(), 1.0);
    assert_eq!(options.y_skew.unwrap(), 0.0);
}

// gtest: Transform.IsPanning
#[test]
fn is_panning() {
    let mut transform = new_transform();

    let mut ease_options = AnimationOptions::new(Duration::from_secs(1));
    let log = EndLog::install(&mut ease_options);
    let check = |transform: &Transform| {
        if transform.in_transition() {
            assert!(transform.is_panning());
        }
        log.take_finish(transform);
    };

    transform.resize(Size::new(1000, 1000)).unwrap();
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { center: Some(ll(0.0, 360.0)), ..Default::default() }, &ease_options);
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    check(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    check(&transform);
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    check(&transform);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    check(&transform);
}

// gtest: Transform.DefaultTransform
#[test]
fn default_transform() {
    let count = |transform: &Transform, kind: fn(&TransformEvent) -> bool| {
        transform.events.iter().filter(|event| kind(event)).count()
    };
    let will_change = |event: &TransformEvent| matches!(event, TransformEvent::CameraWillChange(_));
    let did_change = |event: &TransformEvent| matches!(event, TransformEvent::CameraDidChange(_));

    let mut transform = new_transform();
    assert!(!transform.state().is_valid());

    let null_island = LatLng::default();
    let min: u32 = 0;
    let max: u32 = 65535;

    // Cannot assign invalid sizes.
    let invalid_sizes = [Size::default(), Size::new(min, max), Size::new(max, min)];
    for size in invalid_sizes {
        match transform.resize(size) {
            Ok(()) => panic!("Should throw"),
            Err(_) => assert!(size.is_empty()),
        }
    }

    let valid_size = Size::new(max, max);
    assert!(!valid_size.is_empty());

    transform.resize(valid_size).expect("Should not throw");
    assert_eq!(count(&transform, will_change), 1);
    assert_eq!(count(&transform, did_change), 1);

    assert!(transform.state().is_valid());

    // Double resize
    transform.resize(valid_size).expect("Should not throw");
    assert_eq!(count(&transform, will_change), 1);
    assert_eq!(count(&transform, did_change), 1);

    let center = sc(f64::from(max) / 2.0, f64::from(max) / 2.0);
    let lat_lng = transform.state().view().screen_coordinate_to_lat_lng(&center, WrapMode::Unwrapped).unwrap();
    assert_near(lat_lng.latitude(), null_island.latitude(), 1e-8);
    assert_near(lat_lng.longitude(), null_island.longitude(), 1e-8);

    let point = transform.state().view().lat_lng_to_screen_coordinate(&null_island);
    assert_double_eq(point.x, center.x);
    assert_double_eq(point.y, center.y);

    // Constrain to screen while resizing
    transform.resize(Size::new(1000, 500)).unwrap();
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0))).unwrap();
    transform.set_constrain_mode(ConstrainMode::Screen);

    // Request impossible zoom
    let ease_options = AnimationOptions::new(Duration::from_secs(1));
    let start = start_clock(&mut transform);
    transform
        .ease_to(&CameraOptions { center: Some(ll(56.0, 11.0)), zoom: Some(1.0), ..Default::default() }, &ease_options);
    assert!(transform.in_transition());
    transform.update_transitions(Some(start + Duration::from_millis(250)));

    // Rotate the screen during a transition (resize it)
    transform.resize(Size::new(500, 1000)).unwrap();

    // The resize while constraining to screen should have stopped the transition and updated the state
    assert!(!transform.in_transition());
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 8.22103, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 46.6905, 1e-4);
    assert_near(transform.state().view().scale(), 38.1529, 1e-4);
}

// gtest: Transform.LatLngBounds
#[test]
fn lat_lng_bounds() {
    let null_island = LatLng::default();
    let san_francisco = ll(37.7749, -122.4194);

    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    let max_zoom = transform.state().limits().max_zoom;
    transform.jump_to(&CameraOptions { center: Some(LatLng::default()), zoom: Some(max_zoom), ..Default::default() });

    // Default bounds.
    assert_eq!(transform.state().limits().bounds, LatLngBounds::default());
    assert_eq!(transform.view().center(WrapMode::Wrapped), null_island);

    // Invalid bounds.
    match transform.set_lat_lng_bounds(LatLngBounds::empty()) {
        Ok(()) => panic!("Should throw"),
        Err(_) => assert_eq!(transform.state().limits().bounds, LatLngBounds::default()),
    }

    transform.jump_to(&CameraOptions { center: Some(san_francisco), ..Default::default() });
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), san_francisco.latitude(), 1e-8);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), san_francisco.longitude(), 1e-8);

    // Single location.
    transform.set_lat_lng_bounds(LatLngBounds::singleton(san_francisco)).unwrap();
    assert_eq!(transform.view().center(WrapMode::Wrapped), san_francisco);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃▓▓▓│▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, -180.0), ll(0.0, 180.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(san_francisco), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), san_francisco.longitude());

    // Try crossing the antimeridian from the left.
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -200.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -180.0);

    // Try crossing the antimeridian from the right.
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 200.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Unwrapped).longitude(), 180.0);
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -180.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │▓▓▓┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, 0.0), ll(90.0, 180.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(san_francisco), ..Default::default() });
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), san_francisco.latitude(), 1e-8);
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), 0.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, 0.0), ll(0.0, 180.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(san_francisco), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), 0.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃   │  ▓┃▓  │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │   ┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    let inside = ll(45.0, 150.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(0.0, 120.0), ll(90.0, 240.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(inside), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), inside.longitude());

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 140.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 140.0);

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 160.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 160.0);

    // Constrain latitude only.
    transform.jump_to(&CameraOptions { center: Some(ll(-45.0, inside.longitude())), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), inside.longitude());

    // Crossing the antimeridian, within bounds.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), 181.0)), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), -179.0);

    // Crossing the antimeridian, outside bounds.
    transform.jump_to(&CameraOptions { center: Some(inside), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), 250.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -120.0);

    // Constrain to the left edge.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), 119.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 120.0);

    // Simulate swipe to the left.
    let mut ease_options = AnimationOptions::new(Duration::from_secs(1));
    let log = EndLog::install(&mut ease_options);
    let check_swipe = |transform: &Transform, longitude: f64| {
        if transform.in_transition() {
            assert_near(transform.view().center(WrapMode::Wrapped).longitude(), longitude, 1e-4);
        }
        if log.take_finish(transform) {
            assert_near(transform.view().center(WrapMode::Wrapped).longitude(), longitude, 1e-4);
        }
    };
    let start = start_clock(&mut transform);
    transform.move_by(&sc(-500.0, -500.0), &ease_options).unwrap();

    transform.update_transitions(Some(start));
    check_swipe(&transform, 120.0);
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    check_swipe(&transform, 120.0);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    check_swipe(&transform, 120.0);
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    check_swipe(&transform, 120.0);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    check_swipe(&transform, 120.0);

    // Constrain to the right edge.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), 241.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -120.0);

    // Simulate swipe to the right.
    let start = start_clock(&mut transform);
    transform.move_by(&sc(500.0, 500.0), &ease_options).unwrap();

    transform.update_transitions(Some(start));
    check_swipe(&transform, -120.0);
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    check_swipe(&transform, -120.0);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    check_swipe(&transform, -120.0);
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    check_swipe(&transform, -120.0);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    check_swipe(&transform, -120.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃   │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │  ▓┃▓  │   ┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    let inside = ll(-45.0, -150.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, -240.0), ll(0.0, -120.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(inside), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), inside.longitude());

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -140.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -140.0);

    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -160.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -160.0);

    // Constrain latitude only.
    transform.jump_to(&CameraOptions { center: Some(ll(45.0, inside.longitude())), ..Default::default() });
    assert_eq!(transform.view().center(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), inside.longitude());

    // Crossing the antimeridian, within bounds.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), -181.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.view().center(WrapMode::Wrapped).longitude(), 179.0);

    // Crossing the antimeridian, outside bounds.
    transform.jump_to(&CameraOptions { center: Some(inside), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), -250.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 120.0);

    // Constrain to the left edge.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), -119.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -120.0);

    transform.move_by(&sc(-500.0, 0.0), &AnimationOptions::default()).unwrap();
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), -120.0);

    // Constrain to the right edge.
    transform.jump_to(&CameraOptions { center: Some(ll(inside.latitude(), -241.0)), ..Default::default() });
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 120.0);

    transform.move_by(&sc(500.0, 0.0), &AnimationOptions::default()).unwrap();
    assert_double_eq(transform.view().center(WrapMode::Wrapped).longitude(), 120.0);
}

// Port-only: the floor screen bounds set is the zoom a request below it
// settles on, wherever the center is; the width decides on a square screen
// and the height on a tall one.
#[test]
fn min_zoom_for_screen_bounds() {
    let mut transform = new_transform();
    transform.resize(Size::new(500, 500)).unwrap();
    assert_eq!(None, transform.state().min_zoom_for_screen_bounds());
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0))).unwrap();
    assert_eq!(None, transform.state().min_zoom_for_screen_bounds());
    transform.set_constrain_mode(ConstrainMode::Screen);
    for (size, floor) in [(Size::new(500, 500), 2.81378), (Size::new(500, 1000), 3.65750)] {
        transform.resize(size).unwrap();
        let min_zoom = transform.state().min_zoom_for_screen_bounds().unwrap();
        assert_near(min_zoom, floor, 1e-4);
        for center in [ll(56.0, 11.0), ll(45.0, -5.0)] {
            transform.ease_to(
                &CameraOptions { center: Some(center), zoom: Some(1.0), ..Default::default() },
                &AnimationOptions::default(),
            );
            assert_near(transform.view().zoom(), min_zoom, 1e-9);
        }
    }
}

// gtest: Transform.ConstrainScreenToBounds
#[test]
fn constrain_screen_to_bounds() {
    let mut transform = new_transform();

    transform.resize(Size::new(500, 500)).unwrap();
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0))).unwrap();
    transform.set_constrain_mode(ConstrainMode::Screen);

    // Request impossible zoom
    transform.ease_to(
        &CameraOptions { center: Some(ll(56.0, 11.0)), zoom: Some(1.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().zoom(), 2.81378, 1e-4);

    // Request impossible center left
    transform.ease_to(
        &CameraOptions { center: Some(ll(56.0, -65.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center top
    transform.ease_to(
        &CameraOptions { center: Some(ll(80.0, 11.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 11.0, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 65.88603, 1e-4);

    // Request impossible center right
    transform.ease_to(
        &CameraOptions { center: Some(ll(56.0, 50.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 29.01367, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center bottom
    transform.ease_to(
        &CameraOptions { center: Some(ll(30.0, 11.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 11.0, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 47.89217, 1e-4);

    // Request impossible center with anchor
    transform.ease_to(
        &CameraOptions {
            anchor: Some(sc(250.0, 250.0)),
            center: Some(ll(56.0, -65.0)),
            zoom: Some(4.0),
            ..Default::default()
        },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center (anchor)
    transform.ease_to(
        &CameraOptions { anchor: Some(sc(250.0, 250.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
    );
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Fly to impossible center
    transform.fly_to(
        &CameraOptions { center: Some(ll(56.0, -65.0)), zoom: Some(4.0), ..Default::default() },
        &AnimationOptions::default(),
        false,
    );
    assert_near(transform.view().zoom(), 4.0, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Fly to impossible center and zoom
    transform.fly_to(
        &CameraOptions { center: Some(ll(56.0, -65.0)), zoom: Some(2.0), ..Default::default() },
        &AnimationOptions::default(),
        false,
    );
    assert_near(transform.view().zoom(), 4.0, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 56.0, 1e-4);
}

// gtest: Transform.InvalidPitch
#[test]
fn invalid_pitch() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.view().zoom());
    assert_double_eq(0.0, transform.view().pitch());

    transform.jump_to(&CameraOptions { zoom: Some(1.0), pitch: Some(45.0), ..Default::default() });
    assert_double_eq(1.0, transform.view().zoom());
    assert_double_eq(deg2rad(45.0), transform.view().pitch());

    let invalid = f64::NAN;

    transform.jump_to(&CameraOptions { pitch: Some(invalid), ..Default::default() });
    assert_double_eq(deg2rad(45.0), transform.view().pitch());

    transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    assert_double_eq(deg2rad(60.0), transform.view().pitch());
}

// gtest: Transform.PaddingPreservesMinimumPitch
#[test]
fn padding_preserves_minimum_pitch() {
    for size in [1u32, 64u32] {
        for min_pitch in [0.0, 15.0] {
            // SCOPED_TRACE: shown with the output of a failing test.
            eprintln!("size={size}, minPitch={min_pitch}");
            let mut transform = new_transform();
            transform.resize(Size::new(size, size)).unwrap();
            transform.set_min_pitch(min_pitch);
            transform.set_max_pitch(60.0);
            transform.jump_to(&CameraOptions {
                center: Some(ll(40.7128, -74.006)),
                zoom: Some(9.5),
                pitch: Some(min_pitch),
                ..Default::default()
            });

            transform.jump_to(&CameraOptions {
                padding: Some(insets(f64::from(size) * 28.0, 392.0, 0.0, 0.0)),
                ..Default::default()
            });
            assert_double_eq(deg2rad(min_pitch), transform.view().pitch());

            transform.jump_to(&CameraOptions {
                padding: Some(insets(f64::from(size) * 24.0, 392.0, 0.0, 0.0)),
                ..Default::default()
            });
            assert_double_eq(deg2rad(min_pitch), transform.view().pitch());
            assert_double_eq(f64::from(size) * 24.0, transform.state().view().padding().top());

            transform.resize(Size::new(800, 600)).unwrap();
            assert_double_eq(deg2rad(min_pitch), transform.view().pitch());
            transform.jump_to(&CameraOptions {
                padding: Some(EdgeInsets::default()),
                pitch: Some(45.0),
                ..Default::default()
            });
            assert_double_eq(deg2rad(45.0), transform.view().pitch());
        }
    }
}

// gtest: Transform.MinMaxPitch
#[test]
fn min_max_pitch() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.view().zoom());
    assert_double_eq(0.0, transform.view().pitch());

    transform.jump_to(&CameraOptions { zoom: Some(1.0), pitch: Some(60.0), ..Default::default() });
    assert_double_eq(1.0, transform.view().zoom());
    assert_double_eq(transform.state().limits().max_pitch, transform.view().pitch());
    assert_double_eq(deg2rad(60.0), transform.view().pitch());

    transform.set_max_pitch(70.0);
    transform.jump_to(&CameraOptions { pitch: Some(70.0), ..Default::default() });
    assert_double_eq(transform.state().limits().max_pitch, transform.view().pitch());

    transform.set_max_pitch(45.0);
    transform.jump_to(&CameraOptions { pitch: Some(60.0), ..Default::default() });
    assert_double_eq(transform.state().limits().max_pitch, transform.view().pitch());
    assert_double_eq(deg2rad(45.0), transform.view().pitch());

    transform.jump_to(&CameraOptions { pitch: Some(0.0), ..Default::default() });
    assert_double_eq(transform.state().limits().min_pitch, transform.view().pitch());
    assert_double_eq(0.0, transform.view().pitch());

    transform.set_min_pitch(-10.0);
    transform.jump_to(&CameraOptions { pitch: Some(-10.0), ..Default::default() });
    assert_double_eq(transform.state().limits().min_pitch, transform.view().pitch());
    assert_double_eq(0.0, transform.view().pitch());

    transform.set_min_pitch(15.0);
    transform.jump_to(&CameraOptions { pitch: Some(0.0), ..Default::default() });
    assert_double_eq(transform.state().limits().min_pitch, transform.view().pitch());
    assert_double_eq(deg2rad(15.0), transform.view().pitch());

    transform.set_min_pitch(45.0);
    assert_double_eq(deg2rad(45.0), transform.state().limits().min_pitch);
    transform.set_max_pitch(45.0);
    assert_double_eq(deg2rad(45.0), transform.state().limits().max_pitch);

    transform.set_max_pitch(10.0);
    assert_double_eq(deg2rad(45.0), transform.state().limits().max_pitch);

    transform.set_min_pitch(60.0);
    assert_double_eq(deg2rad(45.0), transform.state().limits().min_pitch);
}

// gtest: Transform.FreeCameraOptionsInvalidSize
#[test]
fn free_camera_options_invalid_size() {
    let mut transform = new_transform();
    let options = FreeCameraOptions {
        orientation: Some(DQuat::from_xyzw(1.0, 1.0, 1.0, 1.0)),
        position: Some(dvec3(0.1, 0.2, 0.3)),
    };
    transform.set_free_camera_options(&options);

    let updated_orientation = transform.view().free_camera_options().orientation.unwrap();
    let updated_position = transform.view().free_camera_options().position.unwrap();

    assert_double_eq(0.0, updated_orientation.x);
    assert_double_eq(0.0, updated_orientation.y);
    assert_double_eq(0.0, updated_orientation.z);
    assert_double_eq(1.0, updated_orientation.w);

    assert_vec3_near_1e5(updated_position, dvec3(0.0, 0.0, 0.0));
}

// gtest: Transform.FreeCameraOptionsNanInput
#[test]
fn free_camera_options_nan_input() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    let mut options = FreeCameraOptions { position: Some(dvec3(0.5, 0.5, 0.25)), ..FreeCameraOptions::default() };
    transform.set_free_camera_options(&options);

    options.position = Some(dvec3(0.0, 0.0, f64::NAN));
    transform.set_free_camera_options(&options);
    assert_eq!(dvec3(0.5, 0.5, 0.25), transform.view().free_camera_options().position.unwrap());

    // Only the invalid parameter should be discarded
    options.position = Some(dvec3(0.3, 0.1, 0.2));
    options.orientation = Some(DQuat::from_xyzw(f64::NAN, 0.0, f64::NAN, 0.0));
    transform.set_free_camera_options(&options);
    assert_vec3_near_1e5(transform.view().free_camera_options().position.unwrap(), dvec3(0.3, 0.1, 0.2));
    assert_eq!(DQuat::IDENTITY, transform.view().free_camera_options().orientation.unwrap());
}

// gtest: Transform.FreeCameraOptionsInvalidZ
#[test]
fn free_camera_options_invalid_z() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    // Invalid z-value (<= 0.0 || > 1) should be clamped to respect both min&max zoom values
    let mut options = FreeCameraOptions { position: Some(dvec3(0.1, 0.1, 0.0)), ..FreeCameraOptions::default() };
    transform.set_free_camera_options(&options);
    assert_double_eq(transform.state().limits().max_zoom, transform.state().view().zoom());
    assert!(transform.view().free_camera_options().position.unwrap()[2] > 0.0);

    options.position = Some(dvec3(0.5, 0.2, 123.456));
    transform.set_free_camera_options(&options);
    assert_double_eq(transform.state().min_zoom(), transform.state().view().zoom());
    assert!(transform.view().free_camera_options().position.unwrap()[2] <= 1.0);
}

// gtest: Transform.CameraFollowsJumpsBelowAFractionalMinimumZoom
#[test]
fn camera_follows_jumps_below_a_fractional_minimum_zoom() {
    let mercator_y = 0.5 - (PI / 4.0 + deg2rad(10.0) / 2.0).tan().ln() / (2.0 * PI);
    for min_zoom in [1.525_373_023_184_723, 1.55, 3.1, 6.2] {
        let _trace = ScopedTrace(std_to_string(min_zoom));
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_min_zoom(min_zoom);
        transform.jump_to(&CameraOptions {
            center: Some(ll(0.0, 0.0)),
            zoom: Some(min_zoom + 2.0),
            ..Default::default()
        });
        transform.jump_to(&CameraOptions { center: Some(ll(10.0, 20.0)), zoom: Some(0.0), ..Default::default() });
        assert!(transform.state().is_valid());
        let position = transform.view().free_camera_options().position.unwrap();
        assert_near(200.0 / 360.0, position[0], 1e-12);
        assert_near(mercator_y, position[1], 1e-12);
    }
}

// gtest: Transform.FreeCameraOptionsHighPitch
#[test]
fn free_camera_options_high_pitch() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    transform.set_max_pitch(180.0);
    let mut options = FreeCameraOptions {
        position: Some(dvec3(0.1, 0.1, 0.1)),
        orientation: Some(DQuat::from_axis_angle(dvec3(1.0, 0.0, 0.0), deg2rad(-90.0))),
    };
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(90.0), transform.state().view().pitch());
    assert_double_eq(0.0, transform.state().view().angle());
    assert_vec3_near_1e5(transform.view().free_camera_options().position.unwrap(), dvec3(0.1, 0.1, 0.1));

    options.position = Some(dvec3(0.1, 0.1, 0.1));
    options.orientation = Some(DQuat::from_axis_angle(dvec3(1.0, 0.0, 0.0), deg2rad(-135.0)));
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(135.0), transform.state().view().pitch());
    assert_double_eq(0.0, transform.state().view().angle());
    assert_vec3_near_1e5(transform.view().free_camera_options().position.unwrap(), dvec3(0.1, 0.1, 0.1));
}

// gtest: Transform.FreeCameraOptionsInvalidOrientation
#[test]
fn free_camera_options_invalid_orientation() {
    // Invalid orientations that cannot be clamped into a valid range
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();

    let mut options =
        FreeCameraOptions { orientation: Some(DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0)), ..FreeCameraOptions::default() };
    transform.set_free_camera_options(&options);
    assert_eq!(Some(DQuat::IDENTITY), transform.view().free_camera_options().orientation);

    // Gimbal lock. Both forward and up vectors are on xy-plane
    transform.set_max_pitch(180.0);
    options.orientation = Some(DQuat::from_axis_angle(dvec3(0.0, 1.0, 0.0), PI / 2.0));
    transform.set_free_camera_options(&options);
    for i in 0..4 {
        assert_near(
            euler(PI / 2.0, PI / 2.0, PI / 2.0).to_array()[i],
            transform.view().free_camera_options().orientation.unwrap().to_array()[i],
            1.0e-12,
        );
    }

    // Camera is upside down
    options.orientation = Some(DQuat::from_axis_angle(dvec3(1.0, 0.0, 0.0), PI / 2.0 + PI / 4.0));
    transform.set_free_camera_options(&options);
    for i in 0..4 {
        assert_near(
            options.orientation.unwrap().to_array()[i],
            transform.view().free_camera_options().orientation.unwrap().to_array()[i],
            1.0e-12,
        );
    }
}

// gtest: Transform.FreeCameraOptionsSetOrientation
#[test]
fn free_camera_options_set_orientation() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    let mut options = FreeCameraOptions { orientation: Some(DQuat::IDENTITY), ..FreeCameraOptions::default() };
    transform.set_free_camera_options(&options);
    assert_double_eq(0.0, transform.state().view().angle());
    assert_double_eq(0.0, transform.state().view().pitch());
    assert_double_eq(0.0, transform.state().view().x());
    assert_double_eq(0.0, transform.state().view().y());

    options.orientation = Some(DQuat::from_axis_angle(dvec3(1.0, 0.0, 0.0), deg2rad(-60.0)));
    transform.set_free_camera_options(&options);
    assert_double_eq(0.0, transform.state().view().angle());
    assert_double_eq(deg2rad(60.0), transform.state().view().pitch());
    assert_double_eq(0.0, transform.state().view().x());
    assert_double_eq(206.0, transform.state().view().y());

    options.orientation = Some(DQuat::from_axis_angle(dvec3(0.0, 0.0, 1.0), deg2rad(56.0)));
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(-56.0), transform.state().view().angle());
    assert_double_eq(0.0, transform.state().view().pitch());
    assert_double_eq(0.0, transform.state().view().roll());
    assert_double_eq(0.0, transform.state().view().x());
    assert_near(152.192_378, transform.state().view().y(), 1e-6);

    options.orientation = Some(euler(0.0, 0.0, deg2rad(-179.0)) * (euler(deg2rad(-30.0), 0.0, 0.0)));
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(179.0), transform.state().view().angle());
    assert_double_eq(deg2rad(30.0), transform.state().view().pitch());
    assert_near(1.308_930, transform.state().view().x(), 1e-6);
    assert_near(56.813_889, transform.state().view().y(), 1e-6);

    options.orientation = Some(
        DQuat::from_axis_angle(dvec3(0.0, 0.0, 1.0), deg2rad(-33.0))
            * (DQuat::from_axis_angle(dvec3(1.0, 0.0, 0.0), deg2rad(-22.0)))
            * (DQuat::from_axis_angle(dvec3(0.0, 0.0, 1.0), deg2rad(11.0))),
    );
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(33.0), transform.state().view().angle());
    assert_double_eq(deg2rad(22.0), transform.state().view().pitch());
    assert_float_eq(deg2rad(11.0), transform.state().view().roll());
}

// gtest: Transform.FreeCameraOptionsClampToBounds
#[test]
#[expect(clippy::approx_constant, reason = "the C++ test's expected values, rounded to six digits")]
fn free_camera_options_clamp_to_bounds() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    transform.set_constrain_mode(ConstrainMode::WidthAndHeight);
    transform.jump_to(&CameraOptions { zoom: Some(8.56), ..Default::default() });
    // Place camera to an arbitrary position looking away from the map
    let options = FreeCameraOptions {
        position: Some(dvec3(-100.0, -10000.0, 1000.0)),
        orientation: Some(euler(deg2rad(-45.0), 0.0, 0.0)),
    };
    transform.set_free_camera_options(&options);

    // Map center should be clamped to width/2 pixels away from map borders
    assert_double_eq(206.0, transform.state().view().x());
    assert_double_eq(206.0, transform.state().view().y());
    assert_double_eq(0.0, transform.state().view().angle());
    assert_double_eq(deg2rad(45.0), transform.state().view().pitch());

    let (right, up, forward) = rotated_frame(transform.view().free_camera_options().orientation.unwrap());
    assert_vec3_near_1e5(
        transform.view().free_camera_options().position.unwrap(),
        dvec3(0.097_656_2, 0.304_816, 0.20716),
    );
    assert_vec3_near_1e5(right, dvec3(1.0, 0.0, 0.0));
    assert_vec3_near_1e5(up, dvec3(0.0, -0.707_107, 0.707_107));
    assert_vec3_near_1e5(forward, dvec3(0.0, -0.707_107, -0.707_107));
}

// gtest: Transform.FreeCameraOptionsInvalidState
#[test]
fn free_camera_options_invalid_state() {
    let transform = new_transform();

    // Invalid size
    assert_double_eq(0.0, transform.state().view().x());
    assert_double_eq(0.0, transform.state().view().y());
    assert_double_eq(0.0, transform.state().view().angle());
    assert_double_eq(0.0, transform.state().view().pitch());

    let options = transform.view().free_camera_options();
    assert_vec3_near_1e5(options.position.unwrap(), dvec3(0.0, 0.0, 0.0));
}

// gtest: Transform.FreeCameraOptionsStateSynchronization
#[test]
fn free_camera_options_state_synchronization() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();

    transform.jump_to(&CameraOptions { pitch: Some(0.0), bearing: Some(0.0), ..Default::default() });
    let (right, up, forward) = rotated_frame(transform.view().free_camera_options().orientation.unwrap());
    assert_vec3_near_1e5(transform.view().free_camera_options().position.unwrap(), dvec3(0.5, 0.5, 0.292_968_75));
    assert_vec3_near_1e5(right, dvec3(1.0, 0.0, 0.0));
    assert_vec3_near_1e5(up, dvec3(0.0, -1.0, 0.0));
    assert_vec3_near_1e5(forward, dvec3(0.0, 0.0, -1.0));

    transform.jump_to(&CameraOptions { center: Some(ll(60.1699, 24.9384)), ..Default::default() });
    assert_vec3_near_1e5(
        transform.view().free_camera_options().position.unwrap(),
        dvec3(0.569_273, 0.289_453, 0.292_969),
    );

    transform.jump_to(&CameraOptions {
        pitch: Some(20.0),
        bearing: Some(77.0),
        center: Some(ll(-20.0, 20.0)),
        ..Default::default()
    });
    assert_vec3_near_1e5(
        transform.view().free_camera_options().position.unwrap(),
        dvec3(0.457_922, 0.57926, 0.275_301),
    );
}

/// The tile matrix as `TransformState::matrix_for` computed it before the
/// projection seam.
fn reference_tile_matrix(scale: f64, tile_id: &UnwrappedTileID) -> DMat4 {
    let tile_scale: u64 = 1u64 << tile_id.canonical.z;
    let s = Projection::world_size(scale) / tile_scale as f64;
    DMat4::from_translation(dvec3(
        (i64::from(tile_id.canonical.x) + i64::from(tile_id.wrap) * tile_scale as i64) as f64 * s,
        f64::from(tile_id.canonical.y) * s,
        0.0,
    )) * DMat4::from_scale(dvec3(s / f64::from(EXTENT), s / f64::from(EXTENT), 1.0))
}

// gtest: TransformState.ProjectionDataMatchesTileMatrix
#[test]
fn projection_data_matches_tile_matrix() {
    let mut transform = new_transform();
    let tile_ids = [
        UnwrappedTileID::new(0, 0, 0),
        UnwrappedTileID::new(1, 0, 1),
        UnwrappedTileID::new(3, 5, 2),
        UnwrappedTileID::new(10, 512, 340),
        UnwrappedTileID::new(14, 16000, 8000),
        UnwrappedTileID::with_wrap(-1, CanonicalTileID::new(2, 1, 1)),
    ];

    for size in [Size::new(1, 1), Size::new(512, 512), Size::new(1024, 768), Size::new(333, 777)] {
        transform.resize(size).unwrap();
        for zoom in [0.0, 2.5, 6.0, 11.75, 16.0] {
            for pitch in [0.0, 30.0, 60.0] {
                for bearing in [0.0, 45.0, 210.0] {
                    for roll in [0.0, 15.0] {
                        transform.jump_to(&CameraOptions {
                            center: Some(ll(37.7749, -122.4194)),
                            zoom: Some(zoom),
                            pitch: Some(pitch),
                            bearing: Some(bearing),
                            roll: Some(roll),
                            ..Default::default()
                        });
                        let state = transform.state();
                        let scale = state.view().scale();

                        for aligned in [false, true] {
                            let proj_matrix = state.view().projection_matrix_with(1.0, aligned);

                            for tile_id in &tile_ids {
                                let tile_matrix = reference_tile_matrix(scale, tile_id);
                                assert_eq!(tile_matrix, state.view().tile_matrix(tile_id));

                                let expected = proj_matrix * tile_matrix;
                                assert_eq!(
                                    expected,
                                    state.view().projection_data_with(tile_id, &proj_matrix).main_matrix
                                );
                            }
                        }

                        for tile_id in &tile_ids {
                            let expected = state.view().projection_matrix() * reference_tile_matrix(scale, tile_id);
                            assert_eq!(expected, state.view().projection_data(tile_id).main_matrix);
                        }
                    }
                }
            }
        }
    }
}

// gtest: TransformState.ProjectionDataMercatorFields
#[test]
fn projection_data_mercator_fields() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(10.0, 20.0)),
        zoom: Some(5.0),
        pitch: Some(20.0),
        bearing: Some(30.0),
        ..Default::default()
    });
    let state = transform.state();

    for tile_id in [
        UnwrappedTileID::new(0, 0, 0),
        UnwrappedTileID::new(3, 5, 2),
        UnwrappedTileID::with_wrap(-1, CanonicalTileID::new(2, 1, 1)),
    ] {
        let data = state.view().projection_data(&tile_id);
        let tile_scale = f64::from(1u32 << tile_id.canonical.z);
        assert_eq!(data.main_matrix, data.fallback_matrix);
        assert_eq!(f64::from(tile_id.canonical.x) / tile_scale, data.tile_mercator_coords[0]);
        assert_eq!(f64::from(tile_id.canonical.y) / tile_scale, data.tile_mercator_coords[1]);
        assert_eq!(1.0 / tile_scale / f64::from(EXTENT), data.tile_mercator_coords[2]);
        assert_eq!(1.0 / tile_scale / f64::from(EXTENT), data.tile_mercator_coords[3]);
        assert_eq!(DVec4::ZERO, data.clipping_plane);
        assert_eq!(0.0, data.projection_transition);
    }
}

// gtest: Transform.GlobeProjectionDefinition
#[test]
fn globe_projection_definition() {
    let mut transform = new_transform();
    assert!(!transform.state().view().is_globe());
    assert_double_eq(0.0, transform.state().view().projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 0.5));
    assert!(transform.state().view().is_globe());
    assert_double_eq(0.5, transform.state().view().projection_transition());
    // The transition is how far the globe has gone, whichever way the
    // definition reads.
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 0.25));
    assert_double_eq(0.75, transform.state().view().projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::between_names("mercator", "vertical-perspective", 0.25));
    assert_double_eq(0.25, transform.state().view().projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert!(!transform.state().view().is_globe());
    assert_double_eq(0.0, transform.state().view().projection_transition());
}

// A transition past either end, from a literal or an overshooting curve,
// holds at that end's projection.
// gtest: Transform.GlobeProjectionTransitionPastEitherEnd
#[test]
fn globe_projection_transition_past_either_end() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::between_names("mercator", "vertical-perspective", 1.5));
    assert_double_eq(1.0, transform.state().view().projection_transition());
    transform.jump_to(&CameraOptions { zoom: Some(1.0), ..Default::default() });
    for step in 1..=3 {
        transform
            .jump_to(&CameraOptions { center: Some(ll(0.0, 40.0 + 120.0 * f64::from(step))), ..Default::default() });
    }
    assert_near(400.0, transform.state().view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    // Past Mercator the globe leaves as it does at 0, so the center wraps back
    // into Mercator's world.
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 1.5));
    assert!(!transform.state().view().is_globe());
    assert_double_eq(0.0, transform.state().view().projection_transition());
    assert_near(40.0, transform.state().view().center(WrapMode::Unwrapped).longitude(), 1e-9);
}

// An animation that crosses into the globe ends where a jump to its camera
// would, at a zoom only the globe allows.
// gtest: Transform.AnimationIntoTheGlobeTakesTheGlobesMinimumZoom
#[test]
fn animation_into_the_globe_takes_the_globes_minimum_zoom() {
    let projection_for_zoom =
        |zoom: f64| ProjectionDefinition::from_name(if zoom < 3.0 { "vertical-perspective" } else { "mercator" });
    // A viewport taller than the world raises Mercator's minimum zoom, which
    // the globe does not have.
    for size in [Size::new(512, 512), Size::new(512, 1024)] {
        for (center, zoom) in [(ll(60.0, 0.0), -0.5), (ll(0.0, 0.0), 0.0)] {
            for fly in [false, true] {
                let _trace = ScopedTrace(format!(
                    "{} to zoom {} at {} on {} px",
                    if fly { "flyTo" } else { "easeTo" },
                    std_to_string(zoom),
                    std_to_string(center.latitude()),
                    size.height
                ));
                let mut transform = new_transform();
                transform.resize(size).unwrap();
                let projection_for_zoom_fn: ProjectionForZoomFn = Rc::new(projection_for_zoom);
                transform.set_projection_for_zoom(Some(projection_for_zoom_fn));
                transform.set_projection_definition(&projection_for_zoom(5.0));
                transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(5.0), ..Default::default() });
                let camera = CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() };
                let animation = AnimationOptions::new(Duration::from_millis(1000));
                let start = start_clock(&mut transform);
                if fly {
                    transform.fly_to(&camera, &animation, false);
                } else {
                    transform.ease_to(&camera, &animation);
                }
                // As the map does each frame, the projection follows the zoom.
                for frame in 1..=20 {
                    transform.update_transitions(Some(start + Duration::from_millis(50 * frame)));
                    let definition = projection_for_zoom(transform.view().zoom());
                    transform.set_projection_definition(&definition);
                }
                assert!(transform.state().view().is_globe());
                assert_near(zoom, transform.view().zoom(), 1e-9);
            }
        }
    }
}

// gtest: VerticalPerspectiveProjection.TileCoordinatesToSphere
#[test]
fn tile_coordinates_to_sphere() {
    let near = |a: &DVec3, b: &DVec3| a.abs_diff_eq(*b, 1e-9);
    let world = UnwrappedTileID::new(0, 0, 0);
    let half = f64::from(EXTENT) / 2.0;
    // (0°, 0°) is the +Z axis; the equator runs through ±X; latitude is Y.
    assert!(near(
        &DVec3::Z,
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, half), &world)
    ));
    assert!(near(
        &DVec3::X,
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half * 1.5, half), &world)
    ));
    assert!(near(
        &DVec3::NEG_X,
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half * 0.5, half), &world)
    ));
    let north = VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, 0.0), &world);
    assert!(north[1] > 0.99);
    let south = VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, f64::from(EXTENT)), &world);
    assert!(south[1] < -0.99);
}

fn set_up_globe(transform: &mut Transform, center: &LatLng, zoom: f64, bearing: f64, pitch: f64) {
    transform.resize(Size::new(800, 600)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&CameraOptions {
        center: Some(*center),
        zoom: Some(zoom),
        bearing: Some(bearing),
        pitch: Some(pitch),
        ..Default::default()
    });
}

/// The GL JS camera tests run on a 512 by 512 globe.
fn set_up_globe_camera(transform: &mut Transform, center: &LatLng, zoom: f64) {
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&CameraOptions { center: Some(*center), zoom: Some(zoom), ..Default::default() });
}

/// One second with a linear easing, so a frame at half time is the frame at
/// k = 0.5.
fn linear_second() -> AnimationOptions {
    AnimationOptions {
        duration: Some(Duration::from_millis(1000)),
        easing: Some(UnitBezier::new(0.0, 0.0, 1.0, 1.0)),
        ..AnimationOptions::default()
    }
}

/// The globe's radius in pixels for `state`'s camera.
fn globe_radius(state: &TransformState) -> f64 {
    VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(state.view().scale()),
        state.view().center(WrapMode::Unwrapped).latitude(),
    )
}

/// Steps the commands started at `start` to `k` seconds in.
fn run_to(transform: &mut Transform, start: Instant, k: f64) {
    transform.update_transitions(Some(start + Duration::from_millis((k * 1000.0) as u64)));
}

/// Steps the commands started at `start` past the end of any of them.
fn run_to_end(transform: &mut Transform, start: Instant) {
    transform.update_transitions(Some(start + Duration::from_hours(24)));
}

// gtest: VerticalPerspectiveProjection.CenterProjectsToScreenCenter
#[test]
fn center_projects_to_screen_center() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(37.0, -122.0), 3.0, 20.0, 30.0);
    let state = transform.state();
    assert!(state.view().is_globe());

    let radius = VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(state.view().scale()),
        state.view().center(WrapMode::Unwrapped).latitude(),
    );
    let m = VerticalPerspectiveProjection::globe_view_projection_matrix(state.view(), radius);
    let center = VerticalPerspectiveProjection::surface_vector(&state.view().center(WrapMode::Unwrapped));
    let surface = center.extend(1.0);
    let clip = m * surface;
    assert_near(0.0, clip[0] / clip[3], 1e-6);
    assert_near(0.0, clip[1] / clip[3], 1e-6);

    let plane = VerticalPerspectiveProjection::clipping_plane(state.view(), radius);
    assert!(plane.dot(surface) > 0.0);
    assert!(plane.dot((-center).extend(1.0)) < 0.0);
}

// gtest: GlobeTransform.CachedViewStateFollowsTheCamera
#[test]
fn cached_view_state_follows_the_camera() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(20.0, -40.0), 2.5, 35.0, 25.0);
    let check = |state: &TransformState| {
        let radius = VerticalPerspectiveProjection::globe_radius_pixels(
            Projection::world_size(state.view().scale()),
            state.view().center(WrapMode::Unwrapped).latitude(),
        );
        let expected = VerticalPerspectiveProjection::globe_view_projection_matrix(state.view(), radius);
        let plane = VerticalPerspectiveProjection::clipping_plane(state.view(), radius);
        let camera = VerticalPerspectiveProjection::camera_position(state.view(), radius);
        let actual = state.view().globe_view_projection_matrix().to_cols_array();
        for (i, &value) in expected.to_cols_array().iter().enumerate() {
            let _trace = ScopedTrace(format!("element {i}"));
            assert_double_eq(value, actual[i]);
        }
        for (i, &value) in plane.to_array().iter().enumerate() {
            let _trace = ScopedTrace(format!("plane {i}"));
            assert_double_eq(value, state.view().globe_clipping_plane()[i]);
        }
        for (i, &value) in camera.to_array().iter().enumerate() {
            let _trace = ScopedTrace(format!("camera {i}"));
            assert_double_eq(value, state.view().globe_camera_position()[i]);
        }
        let round_trip =
            state.view().globe_view_projection_matrix() * state.view().inverse_globe_view_projection_matrix();
        for (i, value) in round_trip.to_cols_array().iter().enumerate() {
            let _trace = ScopedTrace(format!("inverse element {i}"));
            assert_near(if i % 5 == 0 { 1.0 } else { 0.0 }, *value, 1e-9);
        }
    };
    check(transform.state());
    transform.jump_to(&CameraOptions {
        center: Some(ll(-30.0, 100.0)),
        zoom: Some(4.0),
        pitch: Some(50.0),
        bearing: Some(-80.0),
        ..Default::default()
    });
    check(transform.state());
    transform.resize(Size::new(1024, 300)).unwrap();
    check(transform.state());
}

// gtest: GlobeTransform.UnprojectRoundTrip
#[test]
fn unproject_round_trip() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(20.0, -40.0), 2.5, 35.0, 25.0);
    for point in [sc(400.0, 300.0), sc(300.0, 200.0), sc(500.0, 420.0)] {
        let lat_lng = transform.screen_coordinate_to_lat_lng(&point, WrapMode::Wrapped).unwrap();
        let back = transform.lat_lng_to_screen_coordinate(&lat_lng);
        assert_near(point.x, back.x, 1e-6);
        assert_near(point.y, back.y, 1e-6);
    }
    let center = transform.screen_coordinate_to_lat_lng(&sc(400.0, 300.0), WrapMode::Wrapped).unwrap();
    assert_near(20.0, center.latitude(), 1e-9);
    assert_near(-40.0, center.longitude(), 1e-9);
}

// gtest: GlobeTransform.OffGlobePixelsSnapToTheHorizon
#[test]
fn off_globe_pixels_snap_to_the_horizon() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 0.5, 0.0, 0.0);
    // Zoom 0.5 shows the whole globe with room around it; a corner pixel is
    // off the planet.
    let corner = transform.screen_coordinate_to_lat_lng(&sc(0.0, 0.0), WrapMode::Wrapped).unwrap();
    assert!(corner.latitude() < 90.0);
    assert!(corner.latitude() > 0.0);
    assert!(corner.longitude() < 0.0);
    // The snap lands on the horizon: from a camera `d` radii out, that circle
    // is acos(1 / d) from the center.
    let camera = transform.state().view().globe_camera_position();
    let horizon = (1.0 / hypot3(camera[0], camera[1], camera[2])).acos();
    let angle = (deg2rad(corner.latitude()).cos() * deg2rad(corner.longitude()).cos()).acos();
    assert_near(horizon, angle, 1e-6);
}

// gtest: GlobeTransform.MoveByPutsTheDraggedPointAtTheCenter
#[test]
fn move_by_puts_the_dragged_point_at_the_center() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(10.0, 20.0), 3.0, 0.0, 0.0);
    for i in 0..8 {
        let offset = sc(if i % 2 != 0 { 60.0 } else { -45.0 }, if i % 3 != 0 { 30.0 } else { -50.0 });
        let expected =
            transform.screen_coordinate_to_lat_lng(&sc(400.0 - offset.x, 300.0 - offset.y), WrapMode::Wrapped).unwrap();
        let zoom_before = transform.view().zoom();
        let latitude_before = transform.view().center(WrapMode::Wrapped).latitude();
        transform.move_by(&offset, &AnimationOptions::default()).unwrap();
        assert_near(expected.latitude(), transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(expected.longitude(), transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
        // The planet keeps its apparent size: zoom follows the latitude.
        assert_near(
            zoom_before + VerticalPerspectiveProjection::zoom_adjustment(latitude_before, expected.latitude()),
            transform.view().zoom(),
            1e-6,
        );
    }
}

// gtest: GlobeTransform.AnchoredZoomKeepsTheAnchorInPlace
#[test]
fn anchored_zoom_keeps_the_anchor_in_place() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(30.0, 15.0), 3.0, 0.0, 0.0);
    let anchor = sc(250.0, 180.0);
    let anchor_lat_lng = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(4.5), anchor: Some(anchor), ..Default::default() });
    assert_ne!(30.0, transform.view().center(WrapMode::Wrapped).latitude());
    let after = transform.lat_lng_to_screen_coordinate(&anchor_lat_lng);
    assert_near(anchor.x, after.x, 0.5);
    assert_near(anchor.y, after.y, 0.5);
}

// gtest: GlobeTransform.ZoomingOutAroundAnAnchorNearTheHorizonStaysContinuous
#[test]
fn zooming_out_around_an_anchor_near_the_horizon_stays_continuous() {
    // A fast pinch-out with the focal point drifting off the planet: the exact
    // re-anchoring jumped the center by ten degrees of latitude and the zoom
    // by half a level for one frame (a much larger globe); GL JS blends in a
    // damped heuristic near the horizon.
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(48.0, 10.0), 6.0, 0.0, 0.0);
    let anchor = sc(600.0, 200.0);
    let mut last_radius = globe_radius(transform.state());
    let mut last_center = transform.view().center(WrapMode::Wrapped);
    for i in 1..=60 {
        transform.ease_to(
            &CameraOptions { zoom: Some(6.0 - f64::from(i) * 0.1), anchor: Some(anchor), ..Default::default() },
            &AnimationOptions::default(),
        );
        if i % 3 == 0 {
            transform.move_by(&sc(-7.0, 4.0), &AnimationOptions::default()).unwrap();
        }
        let radius = globe_radius(transform.state());
        let center = transform.view().center(WrapMode::Wrapped);
        assert!(radius <= last_radius * 1.005, "step {i}");
        assert!((center.latitude() - last_center.latitude()).abs() < 5.0, "step {i}");
        last_radius = radius;
        last_center = center;
    }
}

// Near the limb the zoom around a point leans on GL JS's damped heuristic,
// which adds up step by step: an animated zoom moves the center about as far
// as the same zoom in one jump.
// gtest: GlobeTransform.AnimatedZoomAroundAPointNearTheLimbMovesLikeAJump
#[test]
fn animated_zoom_around_a_point_near_the_limb_moves_like_a_jump() {
    let anchor = sc(256.0 + 159.0, 256.0);
    let mut jumped = new_transform();
    set_up_globe_camera(&mut jumped, &ll(0.0, 0.0), 1.0);
    jumped.jump_to(&CameraOptions { zoom: Some(2.0), anchor: Some(anchor), ..Default::default() });
    let jump_move = jumped.view().center(WrapMode::Wrapped).longitude();
    assert!(jump_move > 1.0, "{jump_move}");

    let mut animated = new_transform();
    set_up_globe_camera(&mut animated, &ll(0.0, 0.0), 1.0);
    let start = start_clock(&mut animated);
    animated.ease_to(&CameraOptions { zoom: Some(2.0), anchor: Some(anchor), ..Default::default() }, &linear_second());
    for frame in 1..=10 {
        run_to(&mut animated, start, f64::from(frame) / 10.0);
    }
    assert_near(2.0, animated.view().zoom(), 1e-9);
    let animated_move = animated.view().center(WrapMode::Wrapped).longitude();
    assert!(animated_move > 0.75 * jump_move, "{animated_move} {jump_move}");
    assert!(animated_move < 1.25 * jump_move, "{animated_move} {jump_move}");
}

// An animation around an anchor ends where the same change in one jump does,
// zoom included: the planet keeps its size as the anchor carries the center
// to another latitude.
// gtest: GlobeTransform.AnimationsAroundAnAnchorEndLikeAJump
#[test]
fn animations_around_an_anchor_end_like_a_jump() {
    let anchor = sc(256.0, 256.0 - 150.0);
    for rotate in [false, true] {
        let _trace = ScopedTrace(if rotate { "rotate" } else { "zoom" }.to_owned());
        let change = if rotate {
            CameraOptions { bearing: Some(60.0), anchor: Some(anchor), ..Default::default() }
        } else {
            CameraOptions { zoom: Some(4.0), anchor: Some(anchor), ..Default::default() }
        };
        let mut jumped = new_transform();
        set_up_globe_camera(&mut jumped, &ll(45.0, 0.0), 3.0);
        jumped.jump_to(&change);
        assert!((jumped.view().center(WrapMode::Wrapped).latitude() - 45.0).abs() > 0.5);

        let mut animated = new_transform();
        set_up_globe_camera(&mut animated, &ll(45.0, 0.0), 3.0);
        let start = start_clock(&mut animated);
        animated.ease_to(&change, &linear_second());
        for frame in 1..=10 {
            run_to(&mut animated, start, f64::from(frame) / 10.0);
        }
        assert_near(
            jumped.view().center(WrapMode::Wrapped).latitude(),
            animated.view().center(WrapMode::Wrapped).latitude(),
            1e-6,
        );
        assert_near(
            jumped.view().center(WrapMode::Wrapped).longitude(),
            animated.view().center(WrapMode::Wrapped).longitude(),
            1e-6,
        );
        assert_near(jumped.view().zoom(), animated.view().zoom(), 1e-6);
    }
}

// gtest: GlobeTransform.PolesAreReachableAndZoomFollowsLatitude
#[test]
fn poles_are_reachable_and_zoom_follows_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 0.0, 0.0, 0.0);
    let radius_at_equator = globe_radius(transform.state());
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), ..Default::default() });
    assert_near(80.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    // Moving the center towards the pole lowers the zoom by log2(cos 80),
    // below the map's minimum zoom of 0, so the planet keeps its size on
    // screen.
    assert_near(VerticalPerspectiveProjection::zoom_adjustment(0.0, 80.0), transform.view().zoom(), 1e-9);
    assert_near(radius_at_equator, globe_radius(transform.state()), 1e-6);
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(3.0), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), ..Default::default() });
    assert_near(3.0 + VerticalPerspectiveProjection::zoom_adjustment(80.0, 0.0), transform.view().zoom(), 1e-9);

    // Past the Mercator range the center goes as far toward the pole as the
    // zoom it lands on allows, and the zoom follows the edge of the range:
    // the planet keeps its size. A drag over the pole stops where the zoom
    // lets the center go. GL JS: 89 at zoom 1.9907089805355933, then
    // 89.50632407943635.
    let radius_before = globe_radius(transform.state());
    transform.jump_to(&CameraOptions { center: Some(ll(89.0, 0.0)), ..Default::default() });
    assert_near(89.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(1.990_708_980_535_593_3, transform.view().zoom(), 1e-6);
    assert_near(radius_before, globe_radius(transform.state()), 1e-6);
    transform.move_by(&sc(0.0, 300.0), &AnimationOptions::default()).unwrap();
    assert_near(89.506_324_079_436_35, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(radius_before, globe_radius(transform.state()), 1e-6);
}

// gtest: GlobeTransform.BoundsKeepTheCenterInside
#[test]
fn bounds_keep_the_center_inside() {
    // GL JS ignores `maxBounds` on the globe (its TODO); native keeps honoring
    // them, in every constrain mode.
    let bounds = LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0));
    for mode in [ConstrainMode::HeightOnly, ConstrainMode::WidthAndHeight, ConstrainMode::Screen] {
        let _trace = ScopedTrace(format!("{mode:?}"));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(56.0, 11.0), 4.0, 0.0, 0.0);
        transform.set_constrain_mode(mode);
        transform.set_lat_lng_bounds(bounds).unwrap();
        assert_near(56.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(11.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        for outside in [ll(56.0, -65.0), ll(80.0, 11.0), ll(56.0, 50.0), ll(30.0, 11.0)] {
            transform.jump_to(&CameraOptions { center: Some(outside), zoom: Some(4.0), ..Default::default() });
            let center = transform.view().center(WrapMode::Wrapped);
            assert!(bounds.contains(&center), "{}, {}", center.latitude(), center.longitude());
        }
        // A center the whole screen fits around stays where it was asked for.
        transform.jump_to(&CameraOptions { center: Some(ll(55.0, 15.0)), zoom: Some(4.0), ..Default::default() });
        assert_near(55.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(15.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        if mode == ConstrainMode::Screen {
            // The screen has to fit inside the bounds, so zooming out is
            // stopped short.
            transform.jump_to(&CameraOptions { center: Some(ll(56.0, 11.0)), zoom: Some(1.0), ..Default::default() });
            assert!(transform.view().zoom() > 1.0);
            assert!(bounds.contains(&transform.view().center(WrapMode::Wrapped)));
        }
    }
}

// gtest: GlobeTransform.MinZoomFollowsLatitude
#[test]
fn min_zoom_follows_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 2.0, 0.0, 0.0);
    assert_double_eq(0.0, transform.state().min_zoom());
    assert_double_eq(0.0, transform.state().min_zoom_at_latitude(0.0));
    assert_near(-1.0, transform.state().min_zoom_at_latitude(60.0), 1e-12);
    assert_near(deg2rad(80.0).cos().log2(), transform.state().min_zoom_at_latitude(80.0), 1e-12);

    // Zoom requests clamp to the floor at the requested center, not at the
    // equator's.
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(-5.0), ..Default::default() });
    assert_near(transform.state().min_zoom_at_latitude(80.0), transform.view().zoom(), 1e-9);
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(-1.0), ..Default::default() });
    assert_double_eq(0.0, transform.view().zoom());
    assert_eq!(0, transform.state().view().integer_zoom());

    // Integer zoom never wraps below zero.
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(-2.0), ..Default::default() });
    assert_near(-2.0, transform.view().zoom(), 1e-9);
    assert_eq!(0, transform.state().view().integer_zoom());

    // Mercator keeps the plain minimum.
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert_double_eq(transform.state().min_zoom(), transform.state().min_zoom_at_latitude(80.0));
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 0.0)), zoom: Some(-2.0), ..Default::default() });
    assert!(transform.view().zoom() >= 0.0);
}

// gtest: GlobeTransform.CameraFollowsTheGlobeBelowTheMinimumScale
#[test]
fn camera_follows_the_globe_below_the_minimum_scale() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(80.0, 0.0), 8.0, 0.0, 0.0);
    transform.set_min_zoom(6.0);
    transform.jump_to(&CameraOptions { center: Some(ll(80.0, 90.0)), zoom: Some(0.0), ..Default::default() });
    assert_near(transform.state().min_zoom_at_latitude(80.0), transform.view().zoom(), 1e-9);
    assert!(transform.view().zoom() < 6.0);
    assert!(transform.state().is_valid());
    let position = transform.view().free_camera_options().position.unwrap();
    assert_near(0.75, position[0], 1e-12);
    assert_near(0.5 - (PI / 4.0 + deg2rad(80.0) / 2.0).tan().ln() / (2.0 * PI), position[1], 1e-12);
}

// gtest: GlobeTransform.ZoomAdjustment
#[test]
fn zoom_adjustment() {
    assert_double_eq(0.0, VerticalPerspectiveProjection::zoom_adjustment(20.0, 20.0));
    assert_near(-1.0, VerticalPerspectiveProjection::zoom_adjustment(0.0, 60.0), 1e-12);
    assert_near(1.0, VerticalPerspectiveProjection::zoom_adjustment(60.0, 0.0), 1e-12);
    // Past the Mercator range the globe keeps its size at the edge of the
    // range.
    assert_eq!(0.0, VerticalPerspectiveProjection::zoom_adjustment(LATITUDE_MAX, 90.0));
}

// The expected numbers below come from GL JS's own camera (`camera.test.ts`,
// "globe projection" suites, and its `VerticalPerspectiveCameraHelper`) run on
// the same 512 by 512 viewport with a linear easing.

// gtest: GlobeTransform.EaseAdjustsTheZoomToTheNewLatitude
#[test]
fn ease_adjusts_the_zoom_to_the_new_latitude() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 1.0);
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { center: Some(ll(40.0, 0.0)), ..Default::default() }, &linear_second());
    run_to(&mut transform, start, 1.0);
    assert!(!transform.in_transition());
    assert_near(40.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.615_499_999_622_363_8, transform.view().zoom(), 1e-9);
}

// gtest: GlobeTransform.FlyAdjustsTheZoomToTheNewLatitude
#[test]
fn fly_adjusts_the_zoom_to_the_new_latitude() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 1.0);
    let start = start_clock(&mut transform);
    transform.fly_to(&CameraOptions { center: Some(ll(40.0, 0.0)), ..Default::default() }, &linear_second(), false);
    run_to(&mut transform, start, 0.5);
    assert_near(20.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.874_931_581_462_772_6, transform.view().zoom(), 1e-9);
    run_to(&mut transform, start, 1.0);
    assert!(!transform.in_transition());
    assert_near(40.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.615_499_999_622_363_8, transform.view().zoom(), 1e-9);
}

// gtest: GlobeTransform.EaseKeepsThePlanetSizeAcrossLatitudes
#[test]
fn ease_keeps_the_planet_size_across_latitudes() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    let radius = globe_radius(transform.state());
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { center: Some(ll(60.0, 0.0)), ..Default::default() }, &linear_second());
    run_to(&mut transform, start, 0.5);
    assert_near(30.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(2.792_481_250_360_578, transform.view().zoom(), 1e-9);
    assert_near(radius, globe_radius(transform.state()), 1e-6);
    run_to(&mut transform, start, 1.0);
    assert_near(60.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.0, transform.view().zoom(), 1e-9);
    assert_near(radius, globe_radius(transform.state()), 1e-6);
}

// gtest: GlobeTransform.EaseFollowsTheLongitudeSpeedCurve
#[test]
fn ease_follows_the_longitude_speed_curve() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { center: Some(ll(60.0, 60.0)), ..Default::default() }, &linear_second());
    run_to(&mut transform, start, 0.5);
    assert_near(25.026_136_930_423_718, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(30.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.792_481_250_360_578, transform.view().zoom(), 1e-9);
    run_to(&mut transform, start, 1.0);
    assert_near(60.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(60.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.0, transform.view().zoom(), 1e-9);
}

// gtest: GlobeTransform.EaseWithZoomSpeedsUpTheCenter
#[test]
fn ease_with_zoom_speeds_up_the_center() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    let start = start_clock(&mut transform);
    transform.ease_to(
        &CameraOptions { center: Some(ll(60.0, 0.0)), zoom: Some(5.0), ..Default::default() },
        &linear_second(),
    );
    run_to(&mut transform, start, 0.5);
    assert_near(42.426_406_871_192_85, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(4.061_975_143_542_488_5, transform.view().zoom(), 1e-9);
    run_to(&mut transform, start, 1.0);
    assert_near(60.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(5.0, transform.view().zoom(), 1e-9);
}

// gtest: GlobeTransform.FlyFollowsTheGlobePath
#[test]
fn fly_follows_the_globe_path() {
    {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
        let start = start_clock(&mut transform);
        transform.fly_to(
            &CameraOptions { center: Some(ll(60.0, 60.0)), ..Default::default() },
            &linear_second(),
            false,
        );
        run_to(&mut transform, start, 0.5);
        assert_near(25.026_136_930_425_196, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(30.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(1.817_620_920_971_708_5, transform.view().zoom(), 1e-9);
        run_to(&mut transform, start, 1.0);
        assert_near(60.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-8);
        assert_near(60.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(2.0, transform.view().zoom(), 1e-9);
    }
    {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
        let start = start_clock(&mut transform);
        transform.fly_to(
            &CameraOptions { center: Some(ll(60.0, 60.0)), zoom: Some(6.0), ..Default::default() },
            &linear_second(),
            false,
        );
        run_to(&mut transform, start, 0.5);
        assert_near(54.663_652_625_664_554, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(56.470_588_235_293_114, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(2.404_575_237_295_786, transform.view().zoom(), 1e-9);
        run_to(&mut transform, start, 1.0);
        assert_near(60.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-8);
        assert_near(60.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(6.0, transform.view().zoom(), 1e-9);
    }
}

// gtest: GlobeTransform.FlyStaysAboveTheMinimumZoom
#[test]
fn fly_stays_above_the_minimum_zoom() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 20.0);
    let mut animation = linear_second();
    animation.min_zoom = Some(10.0);
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(0.0, 1.0)), zoom: Some(20.0), ..Default::default() },
        &animation,
        false,
    );
    let mut lowest = 20.0;
    for frame in 1..=10 {
        run_to(&mut transform, start, f64::from(frame) / 10.0);
        lowest = f64::min(lowest, transform.view().zoom());
    }
    // GL JS bottoms out at 9.9999993 on the same frames.
    assert!(lowest >= 10.0 - 1e-5, "{lowest}");
    assert!(lowest <= 10.01, "{lowest}");
    assert_near(20.0, transform.view().zoom(), 1e-9);
}

// gtest: GlobeTransform.CameraForBoundsFitsTheGlobe
#[test]
fn camera_for_bounds_fits_the_globe() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 0.0);
    let bounds = LatLngBounds::hull(ll(16.0, -133.0), ll(50.0, -68.0));
    let corners = [bounds.northwest(), bounds.southwest(), bounds.southeast(), bounds.northeast()];

    // The flat fit the globe starts from is 1.4e-8 zoom from GL JS's
    // 2.4694852833012204 on Mercator already, and the globe fit inherits that;
    // the zooms below are GL JS's to within 1.6e-8.
    {
        let mut flat = new_transform();
        flat.resize(Size::new(512, 512)).unwrap();
        flat.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });
        let flat_fit = crate::map::camera_for_lat_lngs(&corners, &flat, &EdgeInsets::default()).unwrap();
        assert!(flat_fit.zoom.is_some());
        assert_near(2.469_485_283_301_220_4, flat_fit.zoom.unwrap(), 1e-7);
    }

    let mut fit = crate::map::camera_for_lat_lngs(&corners, &transform, &EdgeInsets::default()).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717_077_774_070_77, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.496_471_116_313_455, fit.zoom.unwrap(), 1e-7);

    fit = crate::map::camera_for_lat_lngs(&corners, &transform, &insets(15.0, 15.0, 15.0, 15.0)).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717_077_774_070_77, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.398_525_327_285_389, fit.zoom.unwrap(), 1e-7);

    transform.jump_to(&CameraOptions { bearing: Some(45.0), ..Default::default() });
    fit = crate::map::camera_for_lat_lngs(&corners, &transform, &EdgeInsets::default()).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717_077_774_070_77, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.449_918_948_890_396_7, fit.zoom.unwrap(), 1e-7);
}

// Bounds fitted with uneven padding fill the padded part of the view the
// camera shows: every corner and edge midpoint lands inside it and the nearest
// touches its edge.
// gtest: GlobeTransform.CameraForBoundsFitsThePaddedView
#[test]
fn camera_for_bounds_fits_the_padded_view() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 0.0);
    let bounds = LatLngBounds::hull(ll(16.0, -133.0), ll(50.0, -68.0));
    let corners = [bounds.northwest(), bounds.southwest(), bounds.southeast(), bounds.northeast()];
    let mid_latitude = f64::midpoint(bounds.north(), bounds.south());
    let mid_longitude = f64::midpoint(bounds.west(), bounds.east());
    let samples = [
        bounds.northwest(),
        bounds.southwest(),
        bounds.southeast(),
        bounds.northeast(),
        ll(mid_latitude, bounds.west()),
        ll(mid_latitude, bounds.east()),
        ll(bounds.north(), mid_longitude),
        ll(bounds.south(), mid_longitude),
    ];

    for padding in [insets(0.0, 0.0, 200.0, 0.0), insets(150.0, 60.0, 0.0, 0.0)] {
        let _trace =
            ScopedTrace(format!("{} {} {} {}", padding.top(), padding.left(), padding.bottom(), padding.right()));
        let fit = crate::map::camera_for_lat_lngs(&corners, &transform, &padding).unwrap();
        assert!(fit.center.is_some() && fit.zoom.is_some());
        let mut shown = Transform::from_state(transform.state().clone());
        shown.jump_to(&fit);
        let state = shown.state();
        let left = padding.left() / 512.0 * 2.0 - 1.0;
        let right = (512.0 - padding.right()) / 512.0 * 2.0 - 1.0;
        let top = padding.top() / 512.0 * -2.0 + 1.0;
        let bottom = (512.0 - padding.bottom()) / 512.0 * -2.0 + 1.0;
        let mut nearest = f64::INFINITY;
        for sample in &samples {
            let surface = VerticalPerspectiveProjection::surface_vector(sample);
            let clip = state.view().globe_view_projection_matrix() * surface.extend(1.0);
            let x = clip[0] / clip[3];
            let y = clip[1] / clip[3];
            nearest = [x - left, right - x, top - y, y - bottom].into_iter().fold(nearest, f64::min);
        }
        assert_near(0.0, nearest, 1e-3);
    }
}

// gtest: TileProjector.MercatorMatchesTheTileMatrix
#[test]
fn mercator_matches_the_tile_matrix() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(0.0, 0.0)),
        zoom: Some(1.0),
        bearing: Some(30.0),
        pitch: Some(20.0),
        ..Default::default()
    });
    let state = transform.state();
    assert!(!state.view().is_globe());

    // The bottom-right corner of tile 1/0/0 is lat 0, lng 0: the map center.
    let tile_id = UnwrappedTileID::new(1, 0, 0);
    let projector = TileProjector::new(state.view(), &tile_id);
    let corner = Point::new(f64::from(EXTENT), f64::from(EXTENT));
    let pos = state.view().projection_data(&tile_id).main_matrix * DVec4::new(corner.x, corner.y, 0.0, 1.0);

    let projected = projector.project(&corner, 0.0);
    assert_near(pos[0] / pos[3], projected.point.x, 1e-9);
    assert_near(pos[1] / pos[3], projected.point.y, 1e-9);
    assert_near(0.0, projected.point.x, 1e-6);
    assert_near(0.0, projected.point.y, 1e-6);
    assert_double_eq(pos[3], projected.signed_distance_from_camera);
    assert!(!projected.occluded);
    assert!(!projector.project(&Point::new(0.0, 0.0), 0.0).occluded);
    assert_double_eq(1.0, projector.circle_radius_correction());
    assert_double_eq(1.0, projector.pitched_text_correction(&Point::new(100.0, 100.0)));
    assert_double_eq(1.0, state.view().projection().pixel_scale(state.view()));
}

// gtest: TileProjector.GlobeOccludesTheFarSideAndCorrectsForLatitude
#[test]
fn globe_occludes_the_far_side_and_corrects_for_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(40.0, 0.0), 1.0, 0.0, 0.0);
    let state = transform.state();
    let tile_id = UnwrappedTileID::new(1, 0, 0);
    let projector = TileProjector::new(state.view(), &tile_id);

    // The bottom-right corner (lat 0, lng 0) faces the camera; the bottom-left
    // one (lng -180, lat 0) is behind it.
    assert!(!projector.project(&Point::new(f64::from(EXTENT), f64::from(EXTENT)), 0.0).occluded);
    assert!(projector.project(&Point::new(0.0, f64::from(EXTENT)), 0.0).occluded);

    let cos40 = deg2rad(40.0).cos();
    assert_near(cos40, projector.circle_radius_correction(), 1e-12);
    assert_near(1.0 / cos40, state.view().projection().pixel_scale(state.view()), 1e-12);
    // Tile y of latitude 60 in tile 1/0/0.
    let tile_y = Projection::project(&ll(60.0, 0.0), 2.0).y / TILE_SIZE_D * f64::from(EXTENT);
    assert_near(cos40 / deg2rad(60.0).cos(), projector.pitched_text_correction(&Point::new(100.0, tile_y)), 1e-6);
    assert_near(cos40, projector.pitched_text_correction(&Point::new(100.0, f64::from(EXTENT))), 1e-6);
}

// A query on the globe is padded with GL JS's Mercator pitch factor, taken at
// the screen corner's unclamped Mercator position: the values are GL JS's
// `GlobeTransform.maxPitchScaleFactor()` for the same views.
// Map-aligned circles, pitched text and line widths blend their globe size
// with Mercator's through the projection transition: the values are GL JS's
// `GlobeTransform` corrections for the same center and transition.
// gtest: Transform.GlobeSizeCorrectionsBlendThroughTheTransition
#[test]
fn globe_size_corrections_blend_through_the_transition() {
    struct Case {
        latitude: f64,
        transition: f64,
        circle: f64,
        pixel: f64,
        pitched: f64,
    }
    for c in [
        Case {
            latitude: 52.499_167,
            transition: 0.5,
            circle: 0.804_386_481_592_720_2,
            pixel: 1.321_324_254_256_826,
            pitched: 1.263_760_008_806_9,
        },
        Case { latitude: 60.0, transition: 0.25, circle: 0.875, pixel: 1.25, pitched: 1.063_647_309_832_257_2 },
        Case { latitude: 60.0, transition: 1.0, circle: 0.5, pixel: 2.0, pitched: 1.254_589_239_329_028_3 },
        Case { latitude: 0.0, transition: 0.5, circle: 1.0, pixel: 1.0, pitched: 1.754_589_239_329_027_9 },
    ] {
        let mut transform = new_transform();
        transform.resize(Size::new(256, 256)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::between(
            ProjectionType::Mercator,
            ProjectionType::VerticalPerspective,
            c.transition,
        ));
        transform.jump_to(&CameraOptions {
            center: Some(ll(c.latitude, 13.418_056)),
            zoom: Some(11.5),
            ..Default::default()
        });
        let state = transform.state();
        let projection = state.view().projection();
        assert_near(c.circle, projection.circle_radius_correction(state.view()), 1e-9);
        assert_near(c.pixel, projection.pixel_scale(state.view()), 1e-9);
        // An anchor a quarter of the way down the zoom 0 tile, at 66.5 degrees
        // north.
        assert_near(
            c.pitched,
            projection.pitched_text_correction(
                state.view(),
                &Point::new(4096.0, 2048.0),
                &UnwrappedTileID::new(0, 0, 0),
            ),
            1e-9,
        );
    }
}

// gtest: Transform.GlobeMaxPitchScaleFactor
#[test]
fn globe_max_pitch_scale_factor() {
    struct View {
        center: LatLng,
        zoom: f64,
        pitch: f64,
        factor: f64,
    }
    for view in [
        View { center: ll(20.0, 30.0), zoom: 1.0, pitch: 50.0, factor: 1.659_065_925_812_732 },
        View { center: ll(0.0, 0.0), zoom: 1.0, pitch: 60.0, factor: 2.366_025_403_784_242_5 },
        View { center: ll(20.0, 30.0), zoom: 2.0, pitch: 50.0, factor: 1.659_065_925_812_257_8 },
    ] {
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions {
            center: Some(view.center),
            zoom: Some(view.zoom),
            pitch: Some(view.pitch),
            ..Default::default()
        });
        assert_near(view.factor, transform.state().view().max_pitch_scale_factor(), 1e-4);
    }
}

// gtest: Transform.GlobeLocationOcclusion
#[test]
fn globe_location_occlusion() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 1.0, 0.0, 0.0);
    assert!(!transform.is_location_occluded(&ll(0.0, 0.0)));
    assert!(!transform.is_location_occluded(&ll(0.0, 60.0)));
    assert!(!transform.is_location_occluded(&ll(-60.0, 0.0)));
    assert!(transform.is_location_occluded(&ll(0.0, 100.0)));
    assert!(transform.is_location_occluded(&ll(0.0, 180.0)));
    assert!(transform.is_location_occluded(&ll(-30.0, -120.0)));

    set_up_globe(&mut transform, &ll(0.0, 180.0), 1.0, 0.0, 0.0);
    assert!(transform.is_location_occluded(&ll(0.0, 0.0)));
    assert!(!transform.is_location_occluded(&ll(0.0, 180.0)));

    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert!(!transform.is_location_occluded(&ll(0.0, 0.0)));

    let mut unsized_transform = new_transform();
    unsized_transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    assert!(!unsized_transform.is_location_occluded(&ll(0.0, 180.0)));
}

// gtest: Transform.GlobeCenterLongitudeStaysContinuous
#[test]
fn globe_center_longitude_stays_continuous() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 179.0), 1.0, 0.0, 0.0);
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -179.0)), ..Default::default() });
    assert_near(181.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(-179.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    transform.move_by(&sc(-40.0, 0.0), &AnimationOptions::default()).unwrap();
    assert!(transform.view().center(WrapMode::Unwrapped).longitude() > 181.0);
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), ..Default::default() });
    assert_near(179.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);

    // Mercator keeps its wrapped center.
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -179.0)), ..Default::default() });
    assert_near(-179.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
}

// Leaving the globe applies Mercator's limits again: its lowest zoom for the
// view's height and a wrapped center.
// gtest: Transform.LeavingTheGlobeConstrainsTheCamera
#[test]
fn leaving_the_globe_constrains_the_camera() {
    let mut transform = new_transform();
    transform.resize(Size::new(400, 800)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.3), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 120.0)), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -150.0)), ..Default::default() });
    assert_near(210.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(0.3, transform.view().zoom(), 1e-9);

    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert_near((800.0f64 / 512.0).log2(), transform.view().zoom(), 1e-9);
    assert_near(-150.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
}

// Bounds across the antimeridian keep a globe center past 180 degrees, which
// the globe hands over wrapped.
// gtest: Transform.GlobeBoundsAcrossTheAntimeridian
#[test]
fn globe_bounds_across_the_antimeridian() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 150.0), 3.0, 0.0, 0.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 120.0), ll(10.0, 240.0))).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 175.0)), ..Default::default() });
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -175.0)), ..Default::default() });
    assert_near(185.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);

    // A drag east across the antimeridian.
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), ..Default::default() });
    transform.move_by(&sc(-100.0, 0.0), &AnimationOptions::default()).unwrap();
    assert!(transform.view().center(WrapMode::Unwrapped).longitude() > 180.0);
    assert!(transform.view().center(WrapMode::Unwrapped).longitude() < 240.0);
}

// The camera the platforms read carries the wrapped center, whichever copy of
// it the globe keeps. Bounds across the antimeridian keep the center in their
// own range, past 180 degrees: the camera reports it there, so jumping back to
// the reported camera does not move the map, on either projection.
// gtest: Transform.CameraRoundTripsInsideBoundsAcrossTheAntimeridian
#[test]
fn camera_round_trips_inside_bounds_across_the_antimeridian() {
    for globe in [false, true] {
        let _trace = ScopedTrace(if globe { "globe" } else { "mercator" }.to_owned());
        let mut transform = new_transform();
        transform.resize(Size::new(800, 600)).unwrap();
        if globe {
            transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        }
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 180.0)), zoom: Some(5.0), ..Default::default() });
        transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))).unwrap();
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 185.0)), ..Default::default() });
        assert_near(185.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
        let camera = transform.camera_options(None);
        transform.jump_to(&camera);
        assert_near(185.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
        if globe {
            transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
            assert_near(185.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
        }
    }
}

// gtest: Transform.GlobeCameraCenterIsWrapped
#[test]
fn globe_camera_center_is_wrapped() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 179.0), 1.0, 0.0, 0.0);
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, -179.0)), ..Default::default() });
    assert_near(181.0, transform.view().center(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(-179.0, transform.camera_options(None).center.unwrap().longitude(), 1e-9);
}

// The expected centers are GL JS's inertia (handlePanInertia) for the same
// transforms: a fling turns the planet by degrees per pixel at the center's
// latitude, and the zoom follows the latitude to keep the planet's size.
// gtest: Transform.GlobeFlingEndsAtGLJSInertiaCenter
#[test]
fn globe_fling_ends_at_gljs_inertia_center() {
    struct Pan {
        center: LatLng,
        zoom: f64,
        bearing: f64,
        offset: ScreenCoordinate,
        expected: LatLng,
    }
    for pan in [
        Pan {
            center: ll(30.0, -20.0),
            zoom: 4.0,
            bearing: 30.0,
            offset: sc(120.0, -80.0),
            expected: ll(29.646_746_670_134_75, -26.324_743_340_269_5),
        },
        Pan {
            center: ll(-45.0, 170.0),
            zoom: 8.0,
            bearing: -100.0,
            offset: sc(-50.0, 25.0),
            expected: ll(-44.912_800_093_917_55, 169.908_531_669_806_56),
        },
        Pan {
            center: ll(70.0, 0.0),
            zoom: 5.0,
            bearing: 0.0,
            offset: sc(200.0, 0.0),
            expected: ll(70.0, -4.237_578_154_240_85),
        },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(pan.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &pan.center, pan.zoom, pan.bearing, 0.0);
        let start = start_clock(&mut transform);
        transform.move_by(&pan.offset, &linear_second()).unwrap();
        transform.update_transitions(Some(start + Duration::from_millis(1000)));
        assert_near(pan.expected.latitude(), transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(pan.expected.longitude(), transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(
            pan.zoom + VerticalPerspectiveProjection::zoom_adjustment(pan.center.latitude(), pan.expected.latitude()),
            transform.view().zoom(),
            1e-9,
        );
    }
}

// A fling toward a pole eases along its own meridian, as GL JS's inertia
// does, as far toward the pole as the zoom it starts from lets the center go:
// onto the pole from zoom 1.5 on this 800 by 600 view, short of it from zoom
// 3. Moving the location under the end of the fling to the center took it
// over the pole, and the planet spun as it bounced back.
// gtest: Transform.GlobeFlingTowardAPoleStaysOnItsMeridian
#[test]
fn globe_fling_toward_a_pole_stays_on_its_meridian() {
    struct Fling {
        zoom: f64,
        /// GL JS's latitude and zoom halfway and at the end.
        halves: [(f64, f64); 2],
    }
    for fling in [
        Fling { zoom: 1.5, halves: [(75.0, 0.550_015_686_523_503_5), (90.0, -1.035_051_843_768_116_9)] },
        Fling {
            zoom: 3.0,
            halves: [(73.217_160_109_142_3, 2.207_868_201_499_963_8), (86.434_320_218_284_63, 0.464_948_156_231_883_1)],
        },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(fling.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(60.0, 10.0), fling.zoom, 0.0, 0.0);
        let start = start_clock(&mut transform);
        transform.move_by(&sc(0.0, 3000.0), &linear_second()).unwrap();
        let mut latitude = 60.0;
        for frame in 1..=20 {
            transform.update_transitions(Some(start + Duration::from_millis(50 * frame)));
            assert_near(10.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
            assert!(transform.view().center(WrapMode::Wrapped).latitude() >= latitude - 1e-9, "frame {frame}");
            latitude = transform.view().center(WrapMode::Wrapped).latitude();
            if frame % 10 == 0 {
                let (expected_latitude, expected_zoom) = fling.halves[(frame / 10 - 1) as usize];
                assert_near(expected_latitude, latitude, 1e-6);
                assert_near(expected_zoom, transform.view().zoom(), 1e-6);
            }
        }
    }
}

// A fling worth more than half a turn stops at 179.5° the way it was going,
// like GL JS's inertia, rather than easing the shorter way round, against the
// fling.
// gtest: Transform.GlobeFlingStopsShortOfHalfATurn
#[test]
fn globe_fling_stops_short_of_half_a_turn() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 0.5, 0.0, 0.0);
    let start = start_clock(&mut transform);
    transform.move_by(&sc(-5000.0, 0.0), &linear_second()).unwrap();
    let mut longitude = 0.0;
    for frame in 1..=20 {
        transform.update_transitions(Some(start + Duration::from_millis(50 * frame)));
        assert!(transform.view().center(WrapMode::Unwrapped).longitude() >= longitude - 1e-9, "frame {frame}");
        longitude = transform.view().center(WrapMode::Unwrapped).longitude();
    }
    assert_near(179.5, longitude, 1e-9);
    assert_near(0.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
}

// GL JS's drag (`versorSetLocationAtPoint` in globe_utils.ts at a6976b2d2) on a
// 640 by 480 vertical perspective globe, one step at a time from the state GL
// JS had before it. GL JS builds the step's rotation with `quat.fromValues`, a
// Float32Array, which near a pole moves its center by up to 2e-3 degrees a
// step; the expected centers are GL JS's drag run in doubles. The location
// under a pixel differs from GL JS's by a few times 1e-8 degrees, which a step
// carries.
// gtest: GlobeTransform.DragCenterMatchesGLJS
#[test]
fn drag_center_matches_gljs() {
    // (name, center, zoom, bearing, pitch, point, panDelta, expected)
    type Step = (&'static str, (f64, f64), f64, f64, f64, (f64, f64), (f64, f64), (f64, f64));
    let steps: [Step; 30] = [
        (
            "center-drag-east 0",
            (20.0, 10.0),
            1.0,
            0.0,
            0.0,
            (330.0, 240.0),
            (10.0, 0.0),
            (19.965_282_208_712_78, 6.481_528_881_844_27),
        ),
        (
            "center-drag-east 2",
            (19.930_629_843_813_88, 2.963_830_978_065_63),
            1.000_634_558_828_365_6,
            0.0,
            0.0,
            (330.0, 240.0),
            (10.0, 0.0),
            (19.896_042_753_353_06, -0.553_096_790_988_547_6),
        ),
        (
            "center-drag-east 4",
            (19.861_520_785_917_44, -4.069_257_490_790_96),
            1.001_264_349_886_849_2,
            0.0,
            0.0,
            (330.0, 240.0),
            (10.0, 0.0),
            (19.827_063_790_603_2, -7.584_654_172_731_689),
        ),
        (
            "off-center-drag 2",
            (16.074_405_186_537_916, 4.212_860_800_653_175),
            1.032_201_791_216_781_6,
            0.0,
            0.0,
            (400.0, 180.0),
            (6.0, -4.0),
            (14.115_311_037_321_156, 1.360_949_627_755_758_2),
        ),
        (
            "off-center-drag 4",
            (12.158_623_543_717_447, -1.468_300_400_827_388_3),
            1.057_008_507_859_097,
            0.0,
            0.0,
            (400.0, 180.0),
            (6.0, -4.0),
            (10.204_304_619_531_165, -4.278_521_558_340_458),
        ),
        (
            "drag-north-into-pole 0",
            (70.0, -100.0),
            1.0,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (70.737_287_200_838_54, -100.000_000_000_002_05),
        ),
        (
            "drag-north-into-pole 20",
            (84.745_744_016_789_98, -100.000_000_000_000_68),
            -0.901_046_335_291_422_4,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (85.483_031_217_629_15, -100.000_000_000_000_68),
        ),
        (
            "drag-north-into-pole 21",
            (85.483_031_217_629_15, -100.000_000_000_000_68),
            -0.987_205_044_226_623_8,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (86.220_318_418_469_09, -100.000_000_000_000_68),
        ),
        (
            "drag-north-into-pole 27",
            (89.906_754_422_666_92, -100.000_000_000_000_68),
            -0.987_205_044_226_623_8,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (90.0, -100.000_000_000_000_68),
        ),
        (
            "drag-north-into-pole 28",
            (90.0, -100.000_000_000_000_68),
            -0.987_205_044_226_623_8,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (90.0, -100.000_000_000_000_68),
        ),
        (
            "side-drag-at-pole 0",
            (85.051_128_779_806_6, -100.0),
            1.0,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (85.047_765_611_396_3, -101.871_378_175_903_71),
        ),
        (
            "side-drag-at-pole 30",
            (84.951_213_039_195_78, -155.737_980_622_539),
            1.028_764_098_433_647_5,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (84.947_916_719_629_16, -157.581_770_818_180_87),
        ),
        (
            "side-drag-at-pole 59",
            (84.856_481_836_479_73, 151.151_149_744_247_28),
            1.055_512_038_007_472_3,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (84.853_246_506_926_23, 149.332_807_102_710_83),
        ),
        (
            "side-drag-centred-pole 0",
            (89.0, 0.0),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (89.344_388_290_579_13, 13.508_337_366_700_857),
        ),
        (
            "side-drag-centred-pole 2",
            (89.688_776_394_448_31, 22.576_779_263_787_33),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 28.803_865_724_291_313),
        ),
        (
            "side-drag-centred-pole 15",
            (90.0, 83.807_814_056_851_16),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 88.391_476_417_897_82),
        ),
        (
            "side-drag-centred-pole 29",
            (90.0, 147.979_087_111_504_44),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 152.562_749_472_551_1),
        ),
        (
            "drag-off-globe 0",
            (10.0, 5.0),
            1.0,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.982_236_219_514_714, 1.549_461_030_267_593_7),
        ),
        (
            "drag-off-globe 2",
            (9.964_504_640_751_409, -1.900_889_937_549_095_5),
            1.000_157_309_498_887_8,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.946_805_201_839_899, -5.351_053_594_583_845_6),
        ),
        (
            "drag-off-globe 4",
            (9.929_137_841_049_005, -8.801_030_629_313_573),
            1.000_313_481_552_449_5,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.911_502_496_783_827, -12.250_821_727_578_682),
        ),
        (
            "drag-bearing-pitch 0",
            (40.0, 30.0),
            2.5,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.100_686_468_998_106, 30.988_488_451_527_928),
        ),
        (
            "drag-bearing-pitch 4",
            (40.402_655_668_739_54, 33.962_781_163_257_89),
            2.491_431_602_900_159_5,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.503_281_857_387_83, 34.957_178_655_232_61),
        ),
        (
            "drag-bearing-pitch 7",
            (40.704_488_659_029_55, 36.950_467_372_553_02),
            2.484_928_188_992_424_4,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.805_069_184_814_25, 37.949_373_352_371_37),
        ),
        (
            "drag-across-antimeridian 0",
            (0.0, 175.0),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, 177.994_624_839_919_3),
        ),
        (
            "drag-across-antimeridian 5",
            (0.0, -170.026_875_800_407_82),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, -167.032_250_960_488_4),
        ),
        (
            "drag-across-antimeridian 9",
            (0.0, -158.048_376_440_743_3),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, -155.053_751_600_823_1),
        ),
        (
            "drag-south-pole 0",
            (-80.0, 40.0),
            0.8,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-80.493_057_112_485_56, 40.000_000_000_001_48),
        ),
        (
            "drag-south-pole 10",
            (-84.930_571_124_852_39, 40.000_000_000_001_93),
            -0.174_655_743_900_646_53,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-85.423_628_237_337_17, 40.000_000_000_001_93),
        ),
        (
            "drag-south-pole 19",
            (-89.368_085_137_221_91, 40.000_000_000_001_93),
            -0.209_291_019_464_406_45,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-89.861_142_249_709_03, 40.000_000_000_001_93),
        ),
        (
            "drag-south-pole 20",
            (-89.861_142_249_709_03, 40.000_000_000_001_93),
            -0.209_291_019_464_406_45,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-90.0, 40.000_000_000_001_93),
        ),
    ];
    for (name, center, zoom, bearing, pitch, point, pan_delta, expected) in steps {
        let _trace = ScopedTrace(name.to_owned());
        let mut transform = new_transform();
        transform.resize(Size::new(640, 480)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions {
            center: Some(ll(center.0, center.1)),
            zoom: Some(zoom),
            bearing: Some(bearing),
            pitch: Some(pitch),
            ..Default::default()
        });
        let center = VerticalPerspectiveProjection::drag_center(
            transform.state().view(),
            &sc(point.0, 480.0 - point.1),
            &sc(pan_delta.0, -pan_delta.1),
            0.0,
        );
        assert_near(expected.0, center.latitude(), 1e-7);
        assert_near(
            0.0,
            VerticalPerspectiveProjection::difference_of_angles_degrees(expected.1, center.longitude()),
            1e-4,
        );
    }
}

// Dragging sideways at the pole turns the globe around it with the latitude
// held, as GL JS's drag does. The center used to come from the location
// beside it, so it slid away from the pole and the planet spun.
// gtest: GlobeTransform.DragAroundThePoleHoldsItsLatitude
#[test]
fn drag_around_the_pole_holds_its_latitude() {
    let mut transform = new_transform();
    transform.resize(Size::new(640, 480)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&CameraOptions { center: Some(ll(LATITUDE_MAX, -100.0)), zoom: Some(1.0), ..Default::default() });
    for _ in 0..60 {
        transform.move_by_with_anchor(&sc(6.0, 0.0), Some(sc(380.0, 240.0)), &AnimationOptions::default()).unwrap();
    }
    // GL JS after the same 60 steps, run in doubles as in
    // DragCenterMatchesGLJS.
    assert_near(84.853_246_506_926_23, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(
        0.0,
        VerticalPerspectiveProjection::difference_of_angles_degrees(
            149.332_807_102_710_83,
            transform.view().center(WrapMode::Wrapped).longitude(),
        ),
        1e-6,
    );
    assert_near(1.056_416_783_971_354_3, transform.view().zoom(), 1e-6);
}

// Dragging north over the pole stops on it, as GL JS's drag does, the zoom
// holding the planet's size from the edge of the Mercator range on. GL JS
// after the same 40 steps, run in doubles: (90, -100) at zoom
// -0.9872050442266238.
// gtest: GlobeTransform.DragOverThePoleStopsOnIt
#[test]
fn drag_over_the_pole_stops_on_it() {
    let mut transform = new_transform();
    transform.resize(Size::new(640, 480)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&CameraOptions { center: Some(ll(70.0, -100.0)), zoom: Some(1.0), ..Default::default() });
    for _ in 0..40 {
        transform.move_by_with_anchor(&sc(0.0, 6.0), Some(sc(320.0, 300.0)), &AnimationOptions::default()).unwrap();
    }
    assert_eq!(90.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_near(-100.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
    assert_near(-0.987_205_044_226_623_8, transform.view().zoom(), 1e-6);
}

// GL JS's drag without the fixed bearing
// (`versorSetLocationAtPoint(tr, location, anchor, panDelta, false)` in
// globe_utils.ts at 638b3f9ec) on a 640 by 480 globe with a 45 degree field of
// view, run in doubles: one step at a time from the state GL JS had before it,
// the anchor the cursor while it is on the planet and the center otherwise.
// Steps with the cursor off the planet go through the falloff, where native
// and GL JS differ by a few times 1e-7 degrees.
// gtest: GlobeTransform.FreeBearingDragMatchesGLJS
#[test]
fn free_bearing_drag_matches_gljs() {
    // (name, center, zoom, bearing, pitch, anchor, panDelta, expected,
    // expectedBearing)
    type Step = (&'static str, (f64, f64), f64, f64, f64, (f64, f64), (f64, f64), (f64, f64), f64);
    let steps: [Step; 18] = [
        (
            "mid-globe-east 0",
            (20.0, 10.0),
            1.0,
            0.0,
            0.0,
            (340.0, 240.0),
            (10.0, 0.0),
            (19.964_831_035_703_163, 6.458_738_472_277_901),
            -1.210_501_841_080_827_3,
        ),
        (
            "mid-globe-east 7",
            (18.019_206_354_761_504, -16.654_507_328_395_54),
            1.017_184_931_249_020_4,
            -8.825_955_295_711_765,
            0.0,
            (410.0, 240.0),
            (10.0, 0.0),
            (17.292_890_102_626_56, -21.199_373_456_415_287),
            -10.205_107_464_700_406,
        ),
        (
            "over-north-pole 13",
            (79.415_519_352_724_37, -99.999_999_999_999_89),
            0.103_153_326_393_601_52,
            3.410_605_131_648_481e-13,
            0.0,
            (320.0, 284.0),
            (0.0, 6.0),
            (80.146_390_714_591_33, -99.999_999_999_998_07),
            2.046_363_078_989_088_5e-12,
        ),
        (
            "over-north-pole 25",
            (88.492_953_465_698_99, -99.999_999_999_983_97),
            -0.987_205_044_226_624_1,
            1.671_196_514_507_756e-11,
            0.0,
            (320.0, 356.0),
            (0.0, 6.0),
            (89.294_198_664_720_97, -99.999_999_999_962_94),
            3.819_877_747_446_298_6e-11,
        ),
        (
            "over-north-pole 26",
            (89.294_198_664_720_97, -99.999_999_999_962_94),
            -0.987_205_044_226_624_1,
            3.819_877_747_446_298_6e-11,
            0.0,
            (320.0, 362.0),
            (0.0, 6.0),
            (89.894_530_360_047_43, 79.999_999_999_743_75),
            179.999_999_999_746_05,
        ),
        (
            "over-north-pole 27",
            (89.894_530_360_047_43, 79.999_999_999_743_75),
            -0.987_205_044_226_624_1,
            179.999_999_999_746_05,
            0.0,
            (320.0, 368.0),
            (0.0, 6.0),
            (89.072_380_604_509_73, 79.999_999_999_969_99),
            179.999_999_999_972_03,
        ),
        (
            "over-north-pole 39",
            (78.968_703_400_624_05, 79.999_999_999_992_84),
            0.162_096_112_429_317_93,
            179.999_999_999_995,
            0.0,
            (320.0, 440.0),
            (0.0, 6.0),
            (77.908_291_660_738_7, 79.999_999_999_991_59),
            179.999_999_999_993_63,
        ),
        (
            "across-pole-on-globe 19",
            (89.340_072_693_097, -100.000_000_000_017_74),
            -0.009_291_019_464_406_594,
            -1.682_565_198_279_917_2e-11,
            0.0,
            (320.0, 320.0),
            (0.0, 8.0),
            (89.838_065_021_078_38, -100.000_000_000_077_14),
            -7.651_124_178_664_759e-11,
        ),
        (
            "across-pole-on-globe 20",
            (89.838_065_021_078_38, -100.000_000_000_077_14),
            -0.009_291_019_464_406_594,
            -7.651_124_178_664_759e-11,
            0.0,
            (320.0, 328.0),
            (0.0, 8.0),
            (89.661_755_795_874_2, 80.000_000_000_030_92),
            -179.999_999_999_966_58,
        ),
        (
            "across-pole-on-globe 21",
            (89.661_755_795_874_2, 80.000_000_000_030_92),
            -0.009_291_019_464_406_594,
            -179.999_999_999_966_58,
            0.0,
            (320.0, 336.0),
            (0.0, 8.0),
            (89.159_145_086_993_29, 80.000_000_000_013_07),
            -179.999_999_999_984_65,
        ),
        (
            "beside-north-pole 0",
            (85.05, -100.0),
            1.0,
            0.0,
            0.0,
            (386.0, 240.0),
            (6.0, 0.0),
            (85.046_622_784_836_69, -102.121_158_153_155_76),
            -2.113_243_435_839_649,
        ),
        (
            "beside-north-pole 29",
            (82.618_472_178_157_94, -148.045_302_299_421_8),
            1.574_292_954_636_355_8,
            -47.808_141_255_374_27,
            0.0,
            (560.0, 240.0),
            (6.0, 0.0),
            (82.469_763_692_771_44, -149.065_821_126_490_5),
            -48.820_032_972_757_98,
        ),
        (
            "off-globe-and-back 5",
            (59.631_823_345_711_61, 0.0),
            0.015_938_893_735_801_596,
            0.0,
            0.0,
            (320.0, 240.0),
            (0.0, 25.0),
            (68.485_536_141_423_99, 0.0),
            0.0,
        ),
        (
            "off-globe-and-back 9",
            (84.953_325_471_449_74, 180.0),
            -2.506_889_874_048_772,
            180.0,
            0.0,
            (320.0, 240.0),
            (0.0, 25.0),
            (76.099_612_675_744_81, 180.0),
            180.0,
        ),
        (
            "off-globe-and-back 15",
            (84.953_325_471_447_43, 180.0),
            -2.506_889_874_048_112_3,
            180.0,
            0.0,
            (320.0, 240.0),
            (0.0, -25.0),
            (86.192_961_732_847_01, 0.0),
            0.0,
        ),
        (
            "off-globe-and-back 20",
            (33.139_384_526_864_52, 0.0),
            0.743_890_849_416_397_2,
            0.0,
            0.0,
            (320.0, 315.0),
            (0.0, -25.0),
            (20.561_601_743_914_935, 0.0),
            0.0,
        ),
        (
            "bearing-pitch 0",
            (40.0, 30.0),
            1.5,
            30.0,
            20.0,
            (308.0, 255.0),
            (8.0, -5.0),
            (39.905_229_314_899_54, 27.677_367_870_127_796),
            28.548_889_585_142_32,
        ),
        (
            "bearing-pitch 9",
            (36.756_581_940_389_69, 8.665_623_636_743_362),
            1.564_707_449_532_543_5,
            17.020_204_416_245_633,
            20.0,
            (380.0, 210.0),
            (8.0, -5.0),
            (36.007_382_358_240_49, 6.187_079_071_036_919),
            15.610_527_473_047_796,
        ),
    ];
    for (name, center, zoom, bearing, pitch, anchor, pan_delta, expected, expected_bearing) in steps {
        let _trace = ScopedTrace(name.to_owned());
        let mut transform = new_transform();
        transform.resize(Size::new(640, 480)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions {
            center: Some(ll(center.0, center.1)),
            zoom: Some(zoom),
            bearing: Some(bearing),
            pitch: Some(pitch),
            fov: Some(45.0),
            ..Default::default()
        });
        let drag = VerticalPerspectiveProjection::drag_camera(
            transform.state().view(),
            &sc(anchor.0, 480.0 - anchor.1),
            &sc(pan_delta.0, -pan_delta.1),
            0.0,
        );
        assert_near(expected.0, drag.center.latitude(), 1e-6);
        assert_near(
            0.0,
            VerticalPerspectiveProjection::difference_of_angles_degrees(expected.1, drag.center.longitude()),
            1e-6,
        );
        assert_near(
            0.0,
            VerticalPerspectiveProjection::difference_of_angles_degrees(expected_bearing, drag.bearing),
            1e-6,
        );
    }
}

// With a free bearing the location under the cursor stays under it, over the
// pole too, where the bearing turns round; with the bearing held the drag
// stops on the pole and the location stays behind.
// gtest: GlobeTransform.FreeBearingDragKeepsTheLocationUnderTheCursor
#[test]
fn free_bearing_drag_keeps_the_location_under_the_cursor() {
    for free_bearing in [true, false] {
        let _trace = ScopedTrace(if free_bearing { "free bearing" } else { "bearing held" }.to_owned());
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(80.0, -100.0), 1.0, 0.0, 0.0);
        transform.set_globe_free_bearing(free_bearing);
        let mut cursor = sc(400.0, 220.0);
        let mut max_slip = 0.0;
        for _ in 0..40 {
            let grabbed = transform.screen_coordinate_to_lat_lng(&cursor, WrapMode::Wrapped).unwrap();
            cursor.y += 8.0;
            transform.move_by_with_anchor(&sc(0.0, 8.0), Some(cursor), &AnimationOptions::default()).unwrap();
            let point = transform.lat_lng_to_screen_coordinate(&grabbed);
            max_slip = f64::max(max_slip, (point.x - cursor.x).hypot(point.y - cursor.y));
        }
        if free_bearing {
            assert!(max_slip < 1e-6, "{max_slip}");
            assert_near(80.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(180.0, mln_style::math::rad2deg(transform.view().angle()).abs(), 1e-6);
        } else {
            assert!(max_slip > 4.0, "{max_slip}");
            assert_eq!(90.0, transform.view().center(WrapMode::Wrapped).latitude());
        }
    }
}

// Port-only: with the bearing held, the steps of a drag hold the location
// the first of them grabbed, until something else moves the camera. The step
// after that grabs the location then under the cursor, and doesn't bring the
// first one back.
#[test]
fn bearing_held_drag_holds_its_location_until_the_camera_moves() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(30.0, 20.0), 2.0, 0.0, 0.0);
    let slip = |transform: &Transform, location: &LatLng, cursor: &ScreenCoordinate| {
        let point = transform.lat_lng_to_screen_coordinate(location);
        (point.x - cursor.x).hypot(point.y - cursor.y)
    };
    let mut cursor = sc(300.0, 200.0);
    let grabbed = transform.screen_coordinate_to_lat_lng(&cursor, WrapMode::Unwrapped).unwrap();
    for _ in 0..40 {
        cursor = sc(cursor.x + 5.0, cursor.y + 3.0);
        transform.move_by_with_anchor(&sc(5.0, 3.0), Some(cursor), &AnimationOptions::default()).unwrap();
        assert!(slip(&transform, &grabbed, &cursor) < 1e-6);
    }

    transform.jump_to(&CameraOptions { center: Some(ll(10.0, 60.0)), ..Default::default() });
    let regrabbed = transform.screen_coordinate_to_lat_lng(&cursor, WrapMode::Unwrapped).unwrap();
    for _ in 0..10 {
        cursor = sc(cursor.x - 5.0, cursor.y);
        transform.move_by_with_anchor(&sc(-5.0, 0.0), Some(cursor), &AnimationOptions::default()).unwrap();
        assert!(slip(&transform, &regrabbed, &cursor) < 1e-6);
    }
    assert!(slip(&transform, &grabbed, &cursor) > 50.0);
}

// A zoom around a point beside the pole: with the bearing held the center's
// longitude swings towards the point's and the planet turns about the pole,
// as GL JS's does; with a free bearing the planet only grows around the
// point.
// gtest: GlobeTransform.FreeBearingZoomAroundThePoleKeepsTheViewsOrientation
#[test]
fn free_bearing_zoom_around_the_pole_keeps_the_views_orientation() {
    for free_bearing in [true, false] {
        let _trace = ScopedTrace(if free_bearing { "free bearing" } else { "bearing held" }.to_owned());
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(89.0, 0.0), 1.0, 0.0, 0.0);
        transform.set_globe_free_bearing(free_bearing);
        let anchor = sc(520.0, 220.0);
        let location = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
        let beside = transform.screen_coordinate_to_lat_lng(&sc(anchor.x + 20.0, anchor.y), WrapMode::Wrapped).unwrap();
        transform.jump_to(&CameraOptions { zoom: Some(1.6), anchor: Some(anchor), ..Default::default() });
        let point = transform.lat_lng_to_screen_coordinate(&location);
        let beside_point = transform.lat_lng_to_screen_coordinate(&beside);
        let turn = mln_style::math::rad2deg((beside_point.y - point.y).atan2(beside_point.x - point.x));
        if free_bearing {
            assert_near(anchor.x, point.x, 1e-6);
            assert_near(anchor.y, point.y, 1e-6);
            assert_near(0.0, turn, 0.5);
        } else {
            assert!(turn.abs() > 10.0, "{turn}");
        }
    }
}

// A fling over the pole with the bearing free turns the planet on about one
// axis: the center moves away from where it started at every frame, over the
// pole and down the far side, and the bearing turns round. With the bearing
// held, GL JS's fling, it stops on the pole.
// gtest: GlobeTransform.FreeBearingFlingCarriesOnOverThePole
#[test]
fn free_bearing_fling_carries_on_over_the_pole() {
    for free_bearing in [true, false] {
        let _trace = ScopedTrace(if free_bearing { "free bearing" } else { "bearing held" }.to_owned());
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(85.0, -100.0), 1.0, 0.0, 0.0);
        transform.set_globe_free_bearing(free_bearing);
        let start = VerticalPerspectiveProjection::surface_vector(&transform.view().center(WrapMode::Wrapped));
        let begin = start_clock(&mut transform);
        transform.move_by(&sc(0.0, 400.0), &linear_second()).unwrap();
        let mut previous = 0.0;
        for frame in 1..=20 {
            transform.update_transitions(Some(begin + Duration::from_millis(50 * frame)));
            let center = VerticalPerspectiveProjection::surface_vector(&transform.view().center(WrapMode::Wrapped));
            let distance =
                std_clamp(start[0] * center[0] + start[1] * center[1] + start[2] * center[2], -1.0, 1.0).acos();
            assert!(distance >= previous - 1e-9, "frame {frame}");
            previous = distance;
        }
        if free_bearing {
            assert!(transform.view().center(WrapMode::Wrapped).latitude() < 85.0);
            assert_near(80.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(180.0, mln_style::math::rad2deg(transform.view().angle()).abs(), 1e-6);
        } else {
            assert_eq!(90.0, transform.view().center(WrapMode::Wrapped).latitude());
            assert_near(0.0, mln_style::math::rad2deg(transform.view().angle()), 1e-9);
        }
    }
}

// GL JS #8629, its camera at 512 by 512: the center reaches the pole while
// the area past the Mercator range fits in the view, and comes back to the
// edge of the range as a zoom in grows that area past it.
// gtest: GlobeTransform.CenterReachesThePoleAtLowZoom
#[test]
fn center_reaches_the_pole_at_low_zoom() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(60.2, 0.0), 8.0);
    transform.jump_to(&CameraOptions { center: Some(ll(90.0, 0.0)), zoom: Some(0.0), ..Default::default() });
    assert_eq!(90.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_near(0.0, transform.view().zoom(), 1e-9);

    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 0.0);
    transform.jump_to(&CameraOptions { center: Some(ll(90.0, 0.0)), zoom: Some(2.0), ..Default::default() });
    assert_near(88.577_509_403_650_21, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);

    for (zoom, latitude) in [
        (1.0, 90.0),
        (1.5, 90.0),
        (2.0, 88.577_509_403_650_21),
        (2.5, 87.061_492_910_903_57),
        (3.0, 85.989_507_368_491_77),
        (4.0, LATITUDE_MAX),
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(zoom)));
        set_up_globe_camera(&mut transform, &ll(90.0, 0.0), 0.5);
        transform.jump_to(&CameraOptions { zoom: Some(zoom), ..Default::default() });
        assert_near(latitude, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(zoom, transform.view().zoom(), 1e-9);
    }
}

// iOS's quick zoom is anchored on the middle of the view, so with the center
// on the pole it zooms around the pole, a location with no longitude of its
// own. The center keeps its meridian and comes down it once the zoom in brings
// the pole's area past the view: GL JS's latitudes with its camera at 512 by
// 512, the center at 30°E. GL JS turns the planet about the pole towards the
// prime meridian when the center is more than 45° from it; here it keeps the
// meridian there too. A zoom around the pole from below it is GL JS's: the
// center stays put and the pole moves off the anchor.
// gtest: GlobeTransform.ZoomAroundThePoleKeepsTheCentersMeridian
#[test]
fn zoom_around_the_pole_keeps_the_centers_meridian() {
    let gljs_latitudes = [
        90.0,
        90.0,
        90.0,
        90.0,
        90.0,
        90.0,
        89.773_913_827_954_5,
        89.347_172_694_364_29,
        88.949_009_137_878_8,
        88.577_509_403_650_21,
        88.230_887_895_267_78,
        87.907_478_592_370_72,
        87.605_727_042_997_84,
        87.324_182_892_185_6,
        87.061_492_910_903_57,
        86.816_394_491_821_24,
        86.587_709_580_643_92,
        86.374_339_013_848_81,
        86.175_257_235_606_07,
    ];
    for longitude in [30.0, 120.0] {
        let _trace = ScopedTrace(format!("longitude {}", std_to_string(longitude)));
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(90.0, longitude), 1.0);
        let mut zoom = 1.0;
        for latitude in gljs_latitudes {
            zoom += 0.1;
            let _zoom_trace = ScopedTrace(format!("zoom {}", std_to_string(zoom)));
            transform.jump_to(&CameraOptions {
                zoom: Some(zoom),
                anchor: Some(sc(256.0, 256.0)),
                ..Default::default()
            });
            assert_near(longitude, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
            assert_near(latitude, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
        }
    }

    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(88.0, 30.0), 1.5);
    let pole = transform.lat_lng_to_screen_coordinate(&ll(90.0, 0.0));
    transform.jump_to(&CameraOptions { zoom: Some(1.7), anchor: Some(pole), ..Default::default() });
    assert_near(88.0, transform.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(30.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(149.153_336_599_914_16, transform.lat_lng_to_screen_coordinate(&ll(90.0, 0.0)).y, 1e-4);
}

// Without a zoom, GL JS's jump bounds a center past the Mercator range by the
// zoom it lands on, and its ease and flight bound it by the zoom they start
// from (GL JS's camera at 512 by 512).
// gtest: GlobeTransform.JumpBoundsThePoleByTheZoomItLandsOn
#[test]
fn jump_bounds_the_pole_by_the_zoom_it_lands_on() {
    let landing_zoom = -0.512_965_868_530_333_8;
    let mut jump = new_transform();
    set_up_globe_camera(&mut jump, &ll(10.0, 0.0), 3.0);
    jump.jump_to(&CameraOptions { center: Some(ll(89.0, 30.0)), ..Default::default() });
    assert_near(89.0, jump.view().center(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(30.0, jump.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(landing_zoom, jump.view().zoom(), 1e-6);

    let mut ease = new_transform();
    set_up_globe_camera(&mut ease, &ll(10.0, 0.0), 3.0);
    let start = start_clock(&mut ease);
    ease.ease_to(&CameraOptions { center: Some(ll(89.0, 30.0)), ..Default::default() }, &linear_second());
    run_to(&mut ease, start, 0.5);
    assert_near(47.994_753_684_245_89, ease.view().center(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(7.785_022_180_215_378, ease.view().center(WrapMode::Wrapped).longitude(), 1e-6);
    assert_near(2.442_592_415_258_865, ease.view().zoom(), 1e-6);
    run_to(&mut ease, start, 1.0);
    assert_near(85.989_507_368_491_77, ease.view().center(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(30.0, ease.view().center(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(landing_zoom, ease.view().zoom(), 1e-6);

    let mut fly = new_transform();
    set_up_globe_camera(&mut fly, &ll(10.0, 0.0), 3.0);
    let start = start_clock(&mut fly);
    fly.fly_to(&CameraOptions { center: Some(ll(89.0, 30.0)), ..Default::default() }, &linear_second(), false);
    run_to(&mut fly, start, 1.0);
    assert_near(85.989_507_368_491_32, fly.view().center(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(landing_zoom, fly.view().zoom(), 1e-6);
}

// Eases onto, off, and at the pole follow GL JS's camera at 512 by 512,
// halfway and at the end: a zoom in at the pole eases the center back to the
// edge of the Mercator range.
// gtest: GlobeTransform.EasesOntoAndOffThePole
#[test]
fn eases_onto_and_off_the_pole() {
    struct Frame {
        latitude: f64,
        longitude: f64,
        zoom: f64,
    }
    struct Ease {
        name: &'static str,
        start: LatLng,
        start_zoom: f64,
        camera: CameraOptions,
        halves: [Frame; 2],
    }
    for ease in [
        Ease {
            name: "onto the pole",
            start: ll(60.0, 20.0),
            start_zoom: 2.0,
            camera: CameraOptions { center: Some(ll(90.0, 20.0)), zoom: Some(0.0), ..Default::default() },
            halves: [
                Frame { latitude: 78.056_126_393_663_16, longitude: 20.0, zoom: 0.994_903_828_064_923_2 },
                Frame { latitude: 90.0, longitude: 20.0, zoom: 0.0 },
            ],
        },
        Ease {
            name: "onto the pole without a zoom",
            start: ll(60.0, 20.0),
            start_zoom: 1.0,
            camera: CameraOptions { center: Some(ll(89.5, 50.0)), ..Default::default() },
            halves: [
                Frame { latitude: 74.75, longitude: 29.139_640_453_666_175, zoom: 0.073_305_922_392_733_92 },
                Frame { latitude: 89.5, longitude: 50.0, zoom: -1.535_051_843_768_116_9 },
            ],
        },
        Ease {
            name: "off the pole",
            start: ll(90.0, 0.0),
            start_zoom: 0.0,
            camera: CameraOptions { center: Some(ll(30.0, 40.0)), zoom: Some(1.0), ..Default::default() },
            halves: [
                Frame {
                    latitude: 68.786_796_564_403_57,
                    longitude: 24.852_188_049_491_588,
                    zoom: 0.904_706_743_992_299_1,
                },
                Frame { latitude: 30.0, longitude: 40.0, zoom: 1.0 },
            ],
        },
        Ease {
            name: "zooming in at the pole",
            start: ll(90.0, 0.0),
            start_zoom: 0.5,
            camera: CameraOptions { zoom: Some(4.0), ..Default::default() },
            halves: [
                Frame { latitude: 86.500_619_756_682_55, longitude: 0.0, zoom: 2.25 },
                Frame { latitude: LATITUDE_MAX, longitude: 0.0, zoom: 4.0 },
            ],
        },
    ] {
        let _trace = ScopedTrace(ease.name.to_owned());
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ease.start, ease.start_zoom);
        let start = start_clock(&mut transform);
        transform.ease_to(&ease.camera, &linear_second());
        for half in 1..=2 {
            run_to(&mut transform, start, f64::from(half) / 2.0);
            let expected = &ease.halves[(half - 1) as usize];
            assert_near(expected.latitude, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
            assert_near(expected.longitude, transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(expected.zoom, transform.view().zoom(), 1e-6);
        }
    }
}

// Flights land on the pole as GL JS's do (its camera at 512 by 512). From zoom
// 2, GL JS's flight to the pole itself stops at that zoom's limit, as its
// `setLocationAtPoint` has no solution for the pole at the center of the
// view; a flight to any other latitude lands, as this one to 89.9 does, and
// GL JS's own test flies onto the pole from zoom 8.
// gtest: GlobeTransform.FliesOntoThePole
#[test]
fn flies_onto_the_pole() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(60.2, 0.0), 8.0);
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(90.0, 0.0)), zoom: Some(0.0), ..Default::default() },
        &linear_second(),
        false,
    );
    run_to(&mut transform, start, 1.0);
    assert_eq!(90.0, transform.view().center(WrapMode::Wrapped).latitude());
    assert_near(0.0, transform.view().zoom(), 1e-9);

    set_up_globe_camera(&mut transform, &ll(60.0, 20.0), 2.0);
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(89.9, 20.0)), zoom: Some(0.0), ..Default::default() },
        &linear_second(),
        false,
    );
    for (k, latitude, zoom) in [(0.5, 77.690_906_278_984_27, 0.685_984_191_097_479_1), (1.0, 89.9, 0.0)] {
        let _trace = ScopedTrace(format!("k {}", std_to_string(k)));
        run_to(&mut transform, start, k);
        assert_near(latitude, transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(20.0, transform.view().center(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(zoom, transform.view().zoom(), 1e-6);
    }
}

// GL JS's zoom around a point near the pole on an 800 by 600 globe: the center
// goes past the Mercator range as far as the new zoom lets it.
// gtest: GlobeTransform.ZoomAroundNearThePoleMatchesGLJS
#[test]
fn zoom_around_near_the_pole_matches_gljs() {
    struct Zoom {
        center: LatLng,
        zoom: f64,
        zoom_delta: f64,
        anchor: ScreenCoordinate,
        expected: LatLng,
    }
    for zoom in [
        Zoom {
            center: ll(89.0, 0.0),
            zoom: 1.0,
            zoom_delta: 1.0,
            anchor: sc(450.0, 200.0),
            expected: ll(88.224_798_015_387_9, 39.898_924_941_760_924),
        },
        Zoom {
            center: ll(90.0, 0.0),
            zoom: 0.5,
            zoom_delta: 2.0,
            anchor: sc(400.0, 300.0),
            expected: ll(87.690_553_275_798_45, 0.0),
        },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(zoom.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &zoom.center, zoom.zoom, 0.0, 0.0);
        transform.jump_to(&CameraOptions {
            zoom: Some(zoom.zoom + zoom.zoom_delta),
            anchor: Some(zoom.anchor),
            ..Default::default()
        });
        assert_near(zoom.expected.latitude(), transform.view().center(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(zoom.expected.longitude(), transform.view().center(WrapMode::Wrapped).longitude(), 1e-6);
        assert_near(zoom.zoom + zoom.zoom_delta, transform.view().zoom(), 1e-9);
    }
}

// On Mercator a move across the antimeridian during a gesture starts on the
// world copy next to the one it ends on, as it does on main: the camera
// reports that copy, which converts back to the middle of the view.
// gtest: Transform.MercatorCameraCenterIsOnTheTransformsWorldCopy
#[test]
fn mercator_camera_center_is_on_the_transforms_world_copy() {
    let mut transform = new_transform();
    transform.resize(Size::new(800, 600)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 179.0)), zoom: Some(5.0), ..Default::default() });
    let degree = 512.0 * 32.0 / 360.0;
    transform.set_gesture_in_progress(true);
    let start = start_clock(&mut transform);
    transform.move_by(&sc(-2.0 * degree, 0.0), &AnimationOptions::new(Duration::from_secs(10))).unwrap();
    transform.update_transitions(Some(start));
    assert!(transform.view().center(WrapMode::Unwrapped).longitude() < -180.0);
    let center = transform.camera_options(None).center.unwrap();
    assert_near(transform.view().center(WrapMode::Unwrapped).longitude(), center.longitude(), 1e-9);
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(400.0, point.x, 1e-6);
    assert_near(300.0, point.y, 1e-6);
}

// Port-only: from infinite padding, the insets interpolate to NaN
// (inf - inf), for which the C++ EdgeInsets constructor throws out of the
// animation frame (uncaught). The port logs it and keeps the insets.
#[test]
fn padding_interpolated_to_nan_keeps_the_insets() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let infinite = EdgeInsets { top: f64::INFINITY, left: 0.0, bottom: 0.0, right: 0.0 };
    transform.jump_to(&CameraOptions { padding: Some(infinite), ..Default::default() });
    assert_eq!(transform.state().view().padding(), infinite);
    transform.jump_to(&CameraOptions { padding: Some(EdgeInsets::default()), ..Default::default() });
    assert_eq!(transform.state().view().padding(), infinite);
    transform.fly_to(
        &CameraOptions { padding: Some(EdgeInsets::default()), ..Default::default() },
        &AnimationOptions::default(),
        false,
    );
    assert_eq!(transform.state().view().padding(), infinite);
}

// Port-only: a screen coordinate without a LatLng is the C++ LatLng
// constructor's std::domain_error, returned instead of thrown.
#[test]
fn nan_screen_coordinates_are_errors() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let nan = sc(f64::NAN, 500.0);
    let error = transform.screen_coordinate_to_lat_lng(&nan, WrapMode::Wrapped).unwrap_err();
    assert_eq!(error.kind, mln_style::error::ErrorKind::InvalidArgument);
    assert!(error.message.ends_with("must not be NaN"), "{error}");
    let center = transform.view().center(WrapMode::Wrapped);
    assert!(transform.move_by(&nan, &AnimationOptions::default()).is_err());
    assert_eq!(transform.view().center(WrapMode::Wrapped), center);
    // An anchor without a LatLng is ignored (C++ throws).
    transform.jump_to(&CameraOptions { anchor: Some(nan), zoom: Some(2.0), ..Default::default() });
    assert_eq!(transform.view().zoom(), 2.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped), center);
}

// Port-only: the globe returns the errors Mercator does for a NaN point,
// immediate or animated, and ignores a NaN anchor.
#[test]
fn nan_screen_coordinates_are_errors_on_the_globe() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(10.0, 10.0), 3.0);
    let nan = sc(f64::NAN, 0.0);
    let error = transform.screen_coordinate_to_lat_lng(&nan, WrapMode::Wrapped).unwrap_err();
    assert_eq!(error.kind, mln_style::error::ErrorKind::InvalidArgument);
    assert!(error.message.ends_with("must not be NaN"), "{error}");
    let center = transform.view().center(WrapMode::Wrapped);
    assert!(transform.move_by(&nan, &AnimationOptions::default()).is_err());
    assert!(transform.move_by(&nan, &AnimationOptions::new(Duration::from_secs(1))).is_err());
    assert!(transform.move_by(&sc(f64::INFINITY, 0.0), &AnimationOptions::default()).is_err());
    assert_eq!(transform.view().center(WrapMode::Wrapped), center);
    // A NaN anchor is off the planet: the drag turns it around the center.
    transform.move_by_with_anchor(&sc(10.0, 0.0), Some(nan), &AnimationOptions::default()).unwrap();
    assert_ne!(transform.view().center(WrapMode::Wrapped), center);
    let center = transform.view().center(WrapMode::Wrapped);
    transform.jump_to(&CameraOptions { anchor: Some(nan), zoom: Some(2.0), ..Default::default() });
    assert_eq!(transform.view().zoom(), 2.0);
    assert_eq!(transform.view().center(WrapMode::Wrapped), center);
}

// Port-only: before its first resize a globe map moves and answers as
// Mercator does.
#[test]
fn unsized_globe_moves_as_mercator() {
    let mut transform = new_transform();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    let mercator = new_transform();
    assert_eq!(
        transform.screen_coordinate_to_lat_lng(&sc(1.0, 1.0), WrapMode::Wrapped),
        mercator.screen_coordinate_to_lat_lng(&sc(1.0, 1.0), WrapMode::Wrapped),
    );
    transform.move_by(&sc(10.0, 0.0), &AnimationOptions::default()).unwrap();
    transform.jump_to(&CameraOptions { anchor: Some(sc(5.0, 5.0)), zoom: Some(2.0), ..Default::default() });
    assert_eq!(transform.view().zoom(), 2.0);
}

// The tests below port maplibre-native-ffi's patches 0015 (independent
// camera animations) and 0043 (cancel transitions by id).

const LINEAR: UnitBezier = UnitBezier::new(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0);

/// Counts the ends of the commands that start with `options`.
fn counting_end(options: &mut AnimationOptions) -> EndLog {
    EndLog::install(options)
}

fn bearing_degrees(transform: &Transform) -> f64 {
    mln_style::math::rad2deg(-transform.view().angle())
}

// gtest: Transform.IndependentCameraTracks
#[test]
fn independent_camera_tracks() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), center_altitude: Some(120.0), ..Default::default() });
    let mut zoom = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(Duration::from_secs(2)) };
    let zoom_finished = counting_end(&mut zoom);
    let zoom_start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { zoom: Some(12.0), ..Default::default() }, &zoom);
    transform.update_transitions(Some(zoom_start + Duration::from_millis(500)));
    assert_near(transform.view().zoom(), 9.0, 1e-5);

    let mut center = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(Duration::from_secs(8)) };
    let center_finished = counting_end(&mut center);
    let center_start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { center: Some(ll(20.0, 30.0)), ..Default::default() }, &center);
    assert_eq!(zoom_finished.count(&transform), 0);
    transform.update_transitions(Some(zoom_start + Duration::from_secs(2)));
    assert_near(transform.view().zoom(), 12.0, 1e-5);
    assert!(transform.view().center(WrapMode::Wrapped).longitude() > 0.0);
    assert!(transform.view().center(WrapMode::Wrapped).longitude() < 30.0);
    assert_near(transform.state().view().center_altitude(), 120.0, 1e-5);
    assert_eq!(zoom_finished.count(&transform), 1);
    assert_eq!(center_finished.count(&transform), 0);
    assert!(transform.is_panning());
    assert!(!transform.is_scaling());
    assert!(transform.in_transition());
    transform.update_transitions(Some(center_start + Duration::from_secs(8)));
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 20.0, 1e-5);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 30.0, 1e-5);
    assert_near(transform.view().zoom(), 12.0, 1e-5);
    assert_eq!(center_finished.count(&transform), 1);
    assert!(!transform.in_transition());
}

// A timed ease with an easing moves the center along the straight line at
// the easing's pace, as GL JS's easeTo does, rather than at the pace of a
// flight, which is slow, then fast, then slow even at a constant zoom.
#[test]
fn ease_with_an_easing_follows_it() {
    let ease_out = UnitBezier::new(0.0, 0.0, 0.58, 1.0);
    for easing in [LINEAR, ease_out] {
        let mut transform = new_transform();
        transform.resize(Size::new(1000, 1000)).unwrap();
        let from = ll(48.0, 11.0);
        let to = ll(48.002, 11.003);
        transform.jump_to(&CameraOptions { center: Some(from), zoom: Some(15.5), ..Default::default() });
        let scale = transform.state().view().scale();
        let (start_point, end_point) = (Projection::project(&from, scale), Projection::project(&to, scale));
        let animation = AnimationOptions { easing: Some(easing), ..AnimationOptions::new(Duration::from_secs(4)) };
        let start = start_clock(&mut transform);
        transform.ease_to(&CameraOptions { center: Some(to), ..Default::default() }, &animation);
        for quarter in 1..4_u32 {
            let t = f64::from(quarter) / 4.0;
            transform.update_transitions(Some(start + Duration::from_millis(u64::from(quarter) * 1000)));
            let point = Projection::project(&transform.view().center(WrapMode::Unwrapped), scale);
            let expected = easing.solve(t, 0.001);
            assert_near((point.x - start_point.x) / (end_point.x - start_point.x), expected, 1e-5);
            assert_near((point.y - start_point.y) / (end_point.y - start_point.y), expected, 1e-5);
            assert_near(transform.view().zoom(), 15.5, 1e-9);
        }
        transform.update_transitions(Some(start + Duration::from_secs(4)));
        assert!(!transform.in_transition());
        assert_near(transform.view().center(WrapMode::Wrapped).latitude(), to.latitude(), 1e-9);
        assert_near(transform.view().center(WrapMode::Wrapped).longitude(), to.longitude(), 1e-9);
    }
}

// gtest: Transform.PartialReplacementKeepsCommandAndTiming
#[test]
fn partial_replacement_keeps_command_and_timing() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut animation = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(Duration::from_secs(4)) };
    let finished = counting_end(&mut animation);
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { zoom: Some(12.0), bearing: Some(80.0), ..Default::default() }, &animation);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_near(transform.view().zoom(), 9.0, 1e-5);
    assert_near(bearing_degrees(&transform), 20.0, 1e-5);
    transform.jump_to(&CameraOptions { zoom: Some(10.0), ..Default::default() });
    assert_eq!(finished.count(&transform), 0);
    assert!(transform.is_rotating());
    assert!(!transform.is_scaling());
    transform.update_transitions(Some(start + Duration::from_secs(2)));
    assert_near(transform.view().zoom(), 10.0, 1e-5);
    assert_near(bearing_degrees(&transform), 40.0, 1e-5);
    transform.update_transitions(Some(start + Duration::from_secs(4)));
    assert_eq!(finished.count(&transform), 1);
    assert_near(bearing_degrees(&transform), 80.0, 1e-5);
    transform.cancel_transitions();
    assert_eq!(finished.count(&transform), 1);
}

// gtest: Transform.FlightAndAnchorCoupleCenterWithTheirProperties
#[test]
fn flight_and_anchor_couple_center_with_their_properties() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut flight = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(Duration::from_secs(4)) };
    let flight_finished = counting_end(&mut flight);
    let start = start_clock(&mut transform);
    transform.fly_to(
        &CameraOptions { center: Some(ll(20.0, 30.0)), zoom: Some(12.0), roll: Some(20.0), ..Default::default() },
        &flight,
        false,
    );
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_near(mln_style::math::rad2deg(transform.view().roll()), 5.0, 1e-5);
    let center = transform.view().center(WrapMode::Wrapped);
    transform.jump_to(&CameraOptions { zoom: Some(10.0), ..Default::default() });
    assert_eq!(flight_finished.count(&transform), 0); // Roll remains after the coupled flight ends.
    transform.update_transitions(Some(start + Duration::from_secs(4)));
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), center.latitude(), 1e-5);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), center.longitude(), 1e-5);
    assert_near(transform.view().zoom(), 10.0, 1e-5);
    assert_near(mln_style::math::rad2deg(transform.view().roll()), 20.0, 1e-5);
    assert_eq!(flight_finished.count(&transform), 1);

    let mut anchor_animation = AnimationOptions::new(Duration::from_secs(4));
    let anchor_finished = counting_end(&mut anchor_animation);
    let anchor = sc(200.0, 300.0);
    let fixed = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
    let start = start_clock(&mut transform);
    transform
        .ease_to(&CameraOptions { zoom: Some(12.0), anchor: Some(anchor), ..Default::default() }, &anchor_animation);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    let pixel = transform.lat_lng_to_screen_coordinate(&fixed);
    assert_near(pixel.x, anchor.x, 1e-5);
    assert_near(pixel.y, anchor.y, 1e-5);
    transform.jump_to(&CameraOptions { bearing: Some(70.0), ..Default::default() });
    let rotated_pixel = transform.lat_lng_to_screen_coordinate(&fixed);
    assert_near(rotated_pixel.x, anchor.x, 1e-5);
    assert_near(rotated_pixel.y, anchor.y, 1e-5);
    assert_eq!(anchor_finished.count(&transform), 0);
    transform.jump_to(&CameraOptions { center: Some(ll(10.0, 15.0)), ..Default::default() });
    assert_eq!(anchor_finished.count(&transform), 1);
    assert!(!transform.in_transition());
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 10.0, 1e-5);

    transform.ease_to(
        &CameraOptions { center: Some(ll(20.0, 30.0)), anchor: Some(anchor), ..Default::default() },
        &anchor_animation,
    );
    transform.ease_to(
        &CameraOptions { zoom: Some(14.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_secs(2)),
    );
    assert_eq!(anchor_finished.count(&transform), 1); // Explicit center ignores the anchor.
    transform.cancel_transitions();
    assert_eq!(anchor_finished.count(&transform), 2);
}

// gtest: Transform.ComposePaddingPitchAndFieldOfView
#[test]
fn compose_padding_pitch_and_field_of_view() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let animation = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(Duration::from_secs(4)) };
    let start = start_clock(&mut transform);
    transform
        .ease_to(&CameraOptions { padding: Some(insets(300.0, 20.0, 0.0, 0.0)), ..Default::default() }, &animation);
    transform.ease_to(&CameraOptions { pitch: Some(60.0), fov: Some(45.0), ..Default::default() }, &animation);
    let end = start + Duration::from_secs(4);
    transform.update_transitions(Some(end));
    assert_eq!(transform.state().view().padding(), insets(300.0, 20.0, 0.0, 0.0));
    assert_near(mln_style::math::rad2deg(transform.view().fov()), 45.0, 1e-5);
    let max_pitch =
        transform.state().limits().max_pitch + deg2rad(45.0) / 2.0 - (1.3 * (deg2rad(45.0) / 2.0).tan()).atan();
    assert!(max_pitch < deg2rad(60.0));
    assert_near(transform.view().pitch(), max_pitch, 1e-5);
    assert!(!transform.in_transition());
}

// gtest: Transform.CameraCallbacksCanReplaceAndCancelTransitions has no
// port: its callbacks cancel and start commands on the Transform that is
// calling them, which the Rust callbacks can't borrow (see transform.rs).

/// Takes what the transform reported: 0 for each did-change, and the tag of
/// each command's end that `tags` names by transition id.
fn take_tagged(transform: &mut Transform, tags: &[(&EndLog, i32)]) -> Vec<i32> {
    transform
        .drain_events()
        .filter_map(|event| match event {
            TransformEvent::CameraDidChange(_) => Some(0),
            TransformEvent::CommandEnded(end) => {
                tags.iter().find(|(log, _)| end.transition_id == Some(log.transition_id)).map(|(_, tag)| *tag)
            }
            _ => None,
        })
        .collect()
}

// gtest: Transform.PartialCameraCommandsFinishTogetherForStillImage
#[test]
fn partial_camera_commands_finish_together_for_still_image() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    let mut position = AnimationOptions::new(Duration::from_secs(5));
    let position_log = EndLog::install(&mut position);
    let mut bearing = AnimationOptions::new(Duration::from_secs(10));
    let bearing_log = EndLog::install(&mut bearing);
    let tags = [(&position_log, 1), (&bearing_log, 2)];
    transform
        .ease_to(&CameraOptions { center: Some(ll(20.0, 30.0)), zoom: Some(12.0), ..Default::default() }, &position);
    transform.ease_to(&CameraOptions { bearing: Some(60.0), ..Default::default() }, &bearing);
    transform.drain_events().for_each(drop);
    transform.jump_to(&CameraOptions { zoom: Some(10.0), ..Default::default() });
    assert_eq!(take_tagged(&mut transform, &tags), [0]);

    // Static maps advance to the maximum time point for a still image. Each
    // command finishes before its own did-change notification, even when
    // another command has replaced one of its properties.
    transform.update_transitions(None);
    assert_eq!(take_tagged(&mut transform, &tags), [1, 0, 2, 0]);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 20.0, 1e-5);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), 30.0, 1e-5);
    assert_near(transform.view().zoom(), 10.0, 1e-5);
    assert_near(bearing_degrees(&transform), 60.0, 1e-5);
    assert!(!transform.in_transition());
    transform.cancel_transitions();
    let after_cancel = take_tagged(&mut transform, &tags);
    assert!(after_cancel.is_empty(), "{after_cancel:?}");
}

// gtest: Transform.CancelTransitionsByIdLeavesOthersRunning
#[test]
fn cancel_transitions_by_id_leaves_others_running() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut zoom = AnimationOptions { transition_id: Some(1), ..AnimationOptions::new(Duration::from_secs(4)) };
    let zoom_finished = counting_end(&mut zoom);
    let mut bearing = AnimationOptions { transition_id: Some(2), ..AnimationOptions::new(Duration::from_secs(4)) };
    let bearing_finished = counting_end(&mut bearing);
    let start = start_clock(&mut transform);
    transform.ease_to(&CameraOptions { zoom: Some(12.0), ..Default::default() }, &zoom);
    transform.ease_to(&CameraOptions { bearing: Some(80.0), ..Default::default() }, &bearing);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    let zoom_at_cancel = transform.view().zoom();
    let did_changes =
        |transform: &mut Transform| take_changes(transform).into_iter().filter(|(kind, _)| *kind == 'd').count();
    did_changes(&mut transform);

    // Only the command with the identity finishes, once, and the other keeps
    // animating.
    assert!(transform.cancel_transitions_with_id(1));
    assert_eq!(zoom_finished.count(&transform), 1);
    assert_eq!(bearing_finished.count(&transform), 0);
    assert_eq!(did_changes(&mut transform), 1);
    assert!(transform.in_transition());
    assert!(!transform.is_scaling());
    assert!(transform.is_rotating());

    // An identity that no running command carries finishes nothing and
    // reports no change.
    assert!(!transform.cancel_transitions_with_id(1));
    assert!(!transform.cancel_transitions_with_id(3));
    assert_eq!(zoom_finished.count(&transform), 1);
    assert_eq!(did_changes(&mut transform), 0);
    assert!(transform.in_transition());

    transform.update_transitions(Some(start + Duration::from_secs(4)));
    assert_near(transform.view().zoom(), zoom_at_cancel, 1e-5);
    assert_near(bearing_degrees(&transform), 80.0, 1e-5);
    assert_eq!(zoom_finished.count(&transform), 1);
    assert_eq!(bearing_finished.count(&transform), 1);
    assert!(!transform.in_transition());
}

// gtest: Transform.FlyDurationRange
/// maplibre-native-ffi's patch throws for a derived duration that Duration
/// can't hold; the port jumps instead, as a flight past `max_duration` does.
#[test]
fn fly_duration_range() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    let camera = CameraOptions { zoom: Some(1.0), ..Default::default() };
    for velocity in [f64::from_bits(1), 1e-12] {
        transform.jump_to(&CameraOptions { zoom: Some(0.0), ..Default::default() });
        let (animation, ended) = record_end();
        let animation = AnimationOptions { velocity: Some(velocity), ..animation };
        transform.fly_to(&camera, &animation, false);
        assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
        assert_double_eq(transform.view().zoom(), 1.0);
        assert!(!transform.in_transition());
    }

    transform.jump_to(&CameraOptions { zoom: Some(0.0), ..Default::default() });
    let animation = AnimationOptions { velocity: Some(1e-9), ..AnimationOptions::default() };
    transform.fly_to(&camera, &animation, false);
    assert!(transform.in_transition());
    transform.cancel_transitions();

    transform.jump_to(&CameraOptions { zoom: Some(0.0), ..Default::default() });
    let animation = AnimationOptions {
        velocity: Some(f64::from_bits(1)),
        duration: Some(Duration::ZERO),
        ..AnimationOptions::default()
    };
    transform.fly_to(&camera, &animation, false);
    assert_double_eq(transform.view().zoom(), 1.0);
    assert!(!transform.in_transition());
}

/// GL JS's `maxDuration`: a flight longer than it, by a set or a derived
/// duration, jumps; a shorter one flies as without it; an ease ignores it.
#[test]
fn fly_past_max_duration_jumps() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    let target = CameraOptions { center: Some(ll(40.0, -74.0)), zoom: Some(10.0), ..Default::default() };
    let flight = |animation: AnimationOptions| {
        duration_of(move |transform| {
            transform.resize(Size::new(512, 512)).unwrap();
            transform.fly_to(&target, &animation, false);
        })
    };
    let natural = flight(AnimationOptions::default());
    assert!(natural > Duration::from_secs(1));

    // Under the cap: the same flight.
    let capped =
        AnimationOptions { max_duration: Some(natural + Duration::from_millis(1)), ..AnimationOptions::default() };
    assert_eq!(flight(capped), natural);
    let (animation, ended) = record_end();
    transform.fly_to(&target, &AnimationOptions { transition_id: animation.transition_id, ..capped }, false);
    assert!(transform.in_transition());
    transform.cancel_transitions();
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Cancelled));

    // Over it, derived or set: a jump.
    for animation in [
        AnimationOptions { max_duration: natural.checked_sub(Duration::from_millis(1)), ..AnimationOptions::default() },
        AnimationOptions {
            max_duration: Some(Duration::from_secs(1)),
            ..AnimationOptions::new(Duration::from_secs(2))
        },
    ] {
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });
        let (end, ended) = record_end();
        transform.fly_to(&target, &AnimationOptions { transition_id: end.transition_id, ..animation }, false);
        assert!(!transform.in_transition());
        assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
        assert_near(transform.view().zoom(), 10.0, 1e-9);
        assert_near(transform.view().center(WrapMode::Wrapped).latitude(), 40.0, 1e-9);
        assert_near(transform.view().center(WrapMode::Wrapped).longitude(), -74.0, 1e-9);
    }

    // An ease that reuses the flight keeps its duration.
    let ease = duration_of(|transform| {
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(0.0), ..Default::default() });
        transform.ease_to(
            &target,
            &AnimationOptions {
                max_duration: Some(Duration::from_millis(1)),
                ..AnimationOptions::new(Duration::from_secs(2))
            },
        );
    });
    assert_eq!(ease, Duration::from_secs(2));
}

/// The friction decay rate per second for a half-life.
fn friction_rate(half_life: Duration) -> f64 {
    std::f64::consts::LN_2 / half_life.as_secs_f64()
}

fn friction(half_life: Duration, stop_speed: f64) -> MomentumDecay {
    MomentumDecay { model: MomentumDecayModel::Friction { half_life, stop_speed }, ..MomentumDecay::DEFAULT }
}

/// A curve at an even speed, `reference_speed` units per second for
/// `duration` at that speed, whatever the release.
fn even(reference_speed: f64, duration: Duration) -> MomentumDecay {
    let easing = MomentumEasing { x1: 1.0 / 3.0, y1: 1.0 / 3.0, x2: 2.0 / 3.0, y2: 2.0 / 3.0 };
    MomentumDecay {
        model: MomentumDecayModel::Curve { reference_speed, duration, exponent: 0.0, easing },
        ..MomentumDecay::DEFAULT
    }
}

/// A glide from `velocity` that slows by `friction`.
fn glide<T>(velocity: T, decay: MomentumDecay) -> MomentumGlide<T> {
    MomentumGlide { velocity, decay }
}

/// A glide from `velocity` with the C++ header's plain friction, which the
/// ported gtests use, rather than a part's own.
fn plain<T>(velocity: T) -> MomentumGlide<T> {
    glide(velocity, MomentumDecay::DEFAULT)
}

/// A momentum that only pans, from `velocity` by `decay`.
fn panning(velocity: ScreenCoordinate, decay: MomentumDecay) -> CameraMomentum {
    CameraMomentum { pan: Some(glide(velocity, decay)), ..CameraMomentum::default() }
}

/// How long the command that `command` starts on a new transform runs: the
/// least time after its start at which it has ended.
fn duration_of(command: impl Fn(&mut Transform)) -> Duration {
    let runs_at = |elapsed: Duration| {
        let mut transform = new_transform();
        let start = start_clock(&mut transform);
        command(&mut transform);
        transform.update_transitions(Some(start + elapsed));
        transform.in_transition()
    };
    let (mut low, mut high) = (Duration::ZERO, Duration::from_secs(1));
    if !runs_at(low) {
        return low;
    }
    while runs_at(high) {
        low = high;
        high = high + high;
    }
    while high.saturating_sub(low).as_nanos() > 1 {
        let middle = low + high.saturating_sub(low) / 2;
        if runs_at(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    high
}

/// Animation options, and the log of how their command ended.
fn record_end() -> (AnimationOptions, EndLog) {
    let mut animation = AnimationOptions::default();
    let ended = EndLog::install(&mut animation);
    (animation, ended)
}

fn pitch_degrees(transform: &Transform) -> f64 {
    mln_style::math::rad2deg(transform.view().pitch())
}

// gtest: Transform.MomentumSlowsEachPartByFriction
#[test]
fn momentum_slows_each_part_by_friction() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(10.0, 20.0)),
        zoom: Some(8.0),
        pitch: Some(10.0),
        ..Default::default()
    });
    let center = transform.view().center(WrapMode::Wrapped);

    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(glide(1.0, friction(Duration::from_millis(250), 0.01)));
    momentum.bearing.glide = Some(plain(-10.0));
    momentum.pitch.glide = Some(plain(5.0));
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    assert!(transform.in_transition());

    // A speed v that decays at rate k has moved (v / k)(1 - e^(-kt)).
    let zoom_rate = friction_rate(Duration::from_millis(250));
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    assert_near(transform.view().zoom(), 8.0 + 1.0 / zoom_rate * -(-zoom_rate * 0.5).exp_m1(), 1e-9);
    assert!(ended.get(&transform).is_none());

    // Each part stops at its stop speed, so it moves (v - stop) / k.
    let rate = friction_rate(Duration::from_millis(140));
    transform.update_transitions(Some(start + Duration::from_secs(3)));
    assert_near(transform.view().zoom(), 8.0 + (1.0 - 0.01) / zoom_rate, 1e-9);
    assert_near(bearing_degrees(&transform), -(10.0 - 1.0) / rate, 1e-9);
    assert_near(pitch_degrees(&transform), 10.0 + (5.0 - 1.0) / rate, 1e-9);
    assert_near(transform.view().center(WrapMode::Wrapped).latitude(), center.latitude(), 1e-9);
    assert_near(transform.view().center(WrapMode::Wrapped).longitude(), center.longitude(), 1e-9);
    let ended = ended.get(&transform).expect("the momentum ended");
    assert_eq!(ended.outcome, TransitionOutcome::Completed);
    assert_eq!(ended.velocity.zoom, 0.0);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumPansItsDistanceOnScreen
#[test]
fn momentum_pans_its_distance_on_screen() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(10.0, 20.0)), zoom: Some(8.0), ..Default::default() });
    let center = transform.view().center(WrapMode::Wrapped);
    let momentum = panning(sc(1200.0, -500.0), MomentumDecay::DEFAULT);
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    let rate = friction_rate(Duration::from_millis(140));

    transform.update_transitions(Some(start + Duration::from_millis(100)));
    let moved = -(-rate * 0.1).exp_m1() / rate;
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(point.x, 500.0 + 1200.0 * moved, 1e-6);
    assert_near(point.y, 500.0 - 500.0 * moved, 1e-6);

    transform.update_transitions(Some(start + Duration::from_secs(3)));
    let distance = (1300.0 - 1.0) / rate;
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(point.x, 500.0 + distance * 1200.0 / 1300.0, 1e-6);
    assert_near(point.y, 500.0 - distance * 500.0 / 1300.0, 1e-6);
    assert!(!transform.in_transition());
}

// Port-only: momentum pans the globe as a drag around the view center does,
// and its zoom keeps the planet's size as the latitude changes.
#[test]
fn momentum_pans_the_globe_as_a_drag_does() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(10.0, 20.0), 3.0);
    let start = transform.view().center(WrapMode::Wrapped);
    let momentum = panning(sc(0.0, 800.0), MomentumDecay::DEFAULT);
    let begin = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(Some(begin + Duration::from_secs(3)));
    assert!(!transform.in_transition());
    let end = transform.view().center(WrapMode::Wrapped);
    assert!(end.latitude() > start.latitude() + 1.0, "{}", end.latitude());
    assert_near(
        3.0 + VerticalPerspectiveProjection::zoom_adjustment(start.latitude(), end.latitude()),
        transform.view().zoom(),
        1e-9,
    );
}

// Port-only: momentum zoom on the globe keeps to the latitude's minimum
// zoom, which is below the map's toward the poles, as an ease does.
#[test]
fn momentum_zoom_keeps_the_globes_minimum_at_the_latitude() {
    for zoom_velocity in [-3.0, 3.0] {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(80.0, 0.0), -2.0);
        let start = transform.view().zoom();
        assert!(start < 0.0, "{start}");
        let mut momentum = CameraMomentum::default();
        momentum.zoom.glide = Some(plain(zoom_velocity));
        let begin = start_clock(&mut transform);
        transform.start_momentum(&momentum, &AnimationOptions::default());
        transform.update_transitions(Some(begin + Duration::from_millis(16)));
        let zoom = transform.view().zoom();
        let min_zoom = transform.state().min_zoom_at_latitude(transform.view().center(WrapMode::Wrapped).latitude());
        assert!(zoom >= min_zoom - 1e-9, "{zoom_velocity}: {zoom} below {min_zoom}");
        assert!((zoom - start).abs() < 0.5, "{zoom_velocity}: jumped from {start} to {zoom}");
        assert_eq!(zoom > start, zoom_velocity > 0.0, "{zoom_velocity}: {start} -> {zoom}");
    }
}

// gtest: Transform.MomentumFollowsAnEasedGlide. An eased glide follows its
// curve, and its speed is the curve's slope. A straight curve moves at an
// even speed.
#[test]
fn momentum_follows_an_eased_glide() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(glide(-2.0, even(2.0, Duration::from_secs(1))));
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(Some(start + Duration::from_millis(250)));
    assert_near(transform.view().zoom(), 8.0 - 0.5, 1e-6);
    transform.cancel_transitions();
    assert_near(ended.get(&transform).expect("the momentum ended").velocity.zoom, -2.0, 1e-6);

    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_near(transform.view().zoom(), 8.0 - 0.5 - 2.0, 1e-9);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumKeepsItsAnchorInPlace
#[test]
fn momentum_keeps_its_anchor_in_place() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(10.0, 20.0)),
        zoom: Some(8.0),
        pitch: Some(20.0),
        center_altitude: Some(120.0),
        ..Default::default()
    });
    let anchor = sc(800.0, 300.0);
    let ground = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
    let mut momentum = CameraMomentum::default();
    momentum.pitch.glide = Some(plain(20.0));
    momentum.pitch.anchor = Some(anchor);
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(Some(start + Duration::from_secs(3)));
    assert!(pitch_degrees(&transform) > 20.0);
    let point = transform.lat_lng_to_screen_coordinate(&ground);
    // Restoring the altitude after the correction moves the view by far
    // less than a pixel.
    assert_near(point.x, anchor.x, 1e-4);
    assert_near(point.y, anchor.y, 1e-4);
    assert_near(transform.state().view().center_altitude(), 120.0, 1e-6);
}

// gtest: Transform.MomentumCapsItsDistance
#[test]
fn momentum_caps_its_distance() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(glide(50.0, MomentumDecay { max_distance: 2.5, ..MomentumDecay::DEFAULT }));
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(Some(start + Duration::from_secs(3)));
    assert_near(transform.view().zoom(), 10.5, 1e-9);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumReportsItsSpeedWhenCancelled
#[test]
fn momentum_reports_its_speed_when_cancelled() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let mut momentum = panning(sc(1000.0, 0.0), friction(Duration::from_secs(1), 1.0));
    momentum.zoom.glide = Some(glide(1.0, friction(Duration::from_secs(1), 0.01)));
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    transform.cancel_transitions();
    let ended = ended.get(&transform).expect("the momentum ended");
    assert_eq!(ended.outcome, TransitionOutcome::Cancelled);
    // After one half-life, each part moves at half its release speed.
    assert_near(ended.velocity.pan.x, 500.0, 1e-9);
    assert_eq!(ended.velocity.pan.y, 0.0);
    assert_near(ended.velocity.zoom, 0.5, 1e-9);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumPartsGiveWayToCommands
#[test]
fn momentum_parts_give_way_to_commands() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { zoom: Some(8.0), ..Default::default() });
    let mut momentum = panning(sc(1000.0, 0.0), friction(Duration::from_secs(1), 1.0));
    momentum.zoom.glide = Some(glide(1.0, friction(Duration::from_secs(1), 0.01)));
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    assert!(transform.is_panning());
    assert!(transform.is_scaling());

    transform.jump_to(&CameraOptions { zoom: Some(3.0), ..Default::default() });
    assert!(ended.get(&transform).is_none());
    assert!(transform.is_panning());
    assert!(!transform.is_scaling());
    let before = transform.view().center(WrapMode::Wrapped);
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    assert_near(transform.view().zoom(), 3.0, 1e-9);
    assert_ne!(transform.view().center(WrapMode::Wrapped).longitude(), before.longitude());

    transform.jump_to(&CameraOptions { center: Some(ll(10.0, 20.0)), ..Default::default() });
    assert_eq!(ended.get(&transform).expect("the momentum ended").outcome, TransitionOutcome::Replaced);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumSettlesOnTargets
#[test]
fn momentum_settles_on_targets() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { bearing: Some(84.0), ..Default::default() });
    let rate = friction_rate(Duration::from_millis(140));
    // Moves (v - 1) / k = 4 degrees, then settles on the nearest of every 90.
    let mut momentum = CameraMomentum::default();
    momentum.bearing.glide = Some(plain(1.0 + 4.0 * rate));
    momentum.bearing.snap =
        Some(MomentumSnap { spacing: 90.0, tolerance: 7.0, duration: Duration::from_millis(200), offset: 0.0 });
    let mut animation = AnimationOptions::default();
    let finished = counting_end(&mut animation);
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_near(bearing_degrees(&transform), 88.0, 1e-9);
    assert_eq!(finished.count(&transform), 0);
    assert!(transform.is_rotating());
    transform.update_transitions(Some(start + Duration::from_millis(1200)));
    assert_near(bearing_degrees(&transform), 90.0, 1e-9);
    assert_eq!(finished.count(&transform), 1);

    // Without motion, instant snaps settle at once: the bearing around the
    // circle, and the zoom on the nearest whole level. A value outside the
    // tolerance stays.
    let instant =
        |spacing: f64, tolerance: f64| Some(MomentumSnap { spacing, offset: 0.0, tolerance, duration: Duration::ZERO });
    let mut snap_only = CameraMomentum::default();
    snap_only.bearing.snap = instant(360.0, 7.0);
    snap_only.zoom.snap = instant(1.0, 0.5);
    snap_only.pitch.snap = instant(90.0, 7.0);
    transform.jump_to(&CameraOptions {
        bearing: Some(355.0),
        zoom: Some(7.3),
        pitch: Some(30.0),
        ..Default::default()
    });
    transform.start_momentum(&snap_only, &animation);
    assert_eq!(finished.count(&transform), 2);
    assert_near(bearing_degrees(&transform), 0.0, 1e-9);
    assert_near(transform.view().zoom(), 7.0, 1e-9);
    assert_near(pitch_degrees(&transform), 30.0, 1e-9);
    assert!(!transform.in_transition());

    // A spacing that doesn't divide the circle still finds the nearest
    // target around it: 270 is 1 degree from -89.
    let mut uneven = CameraMomentum::default();
    uneven.bearing.snap = instant(270.0, 7.0);
    transform.jump_to(&CameraOptions { bearing: Some(-89.0), ..Default::default() });
    transform.start_momentum(&uneven, &animation);
    assert_near(bearing_degrees(&transform), -90.0, 1e-9);
}

// gtest: Transform.MomentumStopsAPartThatLimitsHold
#[test]
fn momentum_stops_a_part_that_limits_hold() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.set_max_zoom(10.0);
    transform.jump_to(&CameraOptions { zoom: Some(10.0), pitch: Some(0.0), ..Default::default() });
    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(glide(1.0, friction(Duration::from_millis(140), 0.01)));
    momentum.pitch.glide = Some(plain(-20.0));
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(Some(start + Duration::from_millis(20)));
    assert!(!transform.in_transition());
    assert_eq!(ended.get(&transform).expect("the momentum ended").velocity.zoom, 0.0);
    assert_near(transform.view().zoom(), 10.0, 1e-9);
}

// From Orbis's orb-camera (momentum/tests.rs,
// slow_releases_and_bad_decays_do_not_move): the release limits and the
// decays that can't glide, which the gtests don't cover.
#[test]
fn momentum_ignores_slow_releases_and_bad_decays() {
    let pan = MomentumDecay::PAN;
    assert!(GlidePlan::plan(150.0, &pan).is_none());
    assert!(GlidePlan::plan(f64::NAN, &pan).is_none());
    assert!(GlidePlan::plan(f64::INFINITY, &MomentumDecay::DEFAULT).is_none());
    assert!(GlidePlan::plan(100.0, &friction(Duration::ZERO, 1.0)).is_none());
    assert!(GlidePlan::plan(100.0, &friction(Duration::from_millis(100), 0.0)).is_none());
    // A release faster than the cap glides as the cap does.
    let capped = GlidePlan::plan(50_000.0, &pan).unwrap();
    let max = GlidePlan::plan(10_000.0, &pan).unwrap();
    assert_eq!(capped.distance, max.distance);
    assert_eq!(capped.duration, max.duration);
    assert_near(max.distance, 9990.0 / friction_rate(Duration::from_millis(140)), 1e-9);
    // A curve that starts flat would start the glide at rest.
    let flat = MomentumEasing { x1: 0.0, y1: 0.0, x2: 0.25, y2: 1.0 };
    let model = MomentumDecayModel::Curve {
        reference_speed: 1.0,
        duration: Duration::from_secs(1),
        exponent: 0.0,
        easing: flat,
    };
    assert!(GlidePlan::plan(100.0, &MomentumDecay { model, ..MomentumDecay::DEFAULT }).is_none());
    // A curve whose glide would shrink as the release speeds up is out of
    // range.
    let MomentumDecayModel::Curve { reference_speed, duration, easing, .. } = MomentumDecay::ANDROID_FLING.model else {
        unreachable!()
    };
    for exponent in [-1.0, -2.0] {
        let model = MomentumDecayModel::Curve { reference_speed, duration, exponent, easing };
        let decay = MomentumDecay { model, ..MomentumDecay::ANDROID_FLING };
        assert!(decay.validate().is_err(), "{exponent}");
        assert!(GlidePlan::plan(1000.0, &decay).is_none(), "{exponent}");
    }
}

// Android's fling, as its `OverScroller` plans it: the curve model with the
// preset's numbers gives the same distance and duration.
#[test]
fn the_android_fling_preset_follows_androids_spline() {
    let fling = MomentumDecay::ANDROID_FLING;
    // A 1000 dp/s fling travels 194.3 dp over 555.2 ms, a 300 dp/s one
    // 24.0 dp over 228.8 ms, and a 4000 dp/s one 2157 dp over 1541 ms.
    for (speed, distance, ms) in [(1000.0, 194.314, 555.182), (300.0, 24.024, 228.800), (4000.0, 2156.95, 1540.68)] {
        let glide = GlidePlan::plan(speed, &fling).unwrap();
        assert_near(glide.distance, distance, distance * 1e-4);
        assert_near(glide.duration.as_secs_f64() * 1000.0, ms, ms * 1e-4);
        assert_eq!(glide.easing, Some(MomentumEasing::ANDROID_FLING));
        // It starts at the release speed.
        assert_near(glide.distance * glide.speed(Duration::ZERO), speed, speed * 1e-3);
    }
    assert!(GlidePlan::plan(40.0, &fling).is_none());
    // A release that would glide past the maximum distance glides it, at
    // the speed that does.
    let short = GlidePlan::plan(1000.0, &MomentumDecay { max_distance: 24.024, ..fling }).unwrap();
    assert_near(short.distance, 24.024, 1e-9);
    assert_near(short.duration.as_secs_f64() * 1000.0, 228.800, 0.01);
}

#[test]
fn the_pans_default_friction_limits_its_release() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(8.0), ..Default::default() });

    // Slower than the pan's 200 px/s minimum: no glide.
    transform.start_momentum(&panning(sc(150.0, 0.0), MomentumDecay::PAN), &AnimationOptions::default());
    assert!(!transform.in_transition());

    // Faster than its 10 000 px/s cap glides as the cap does.
    let glide = |speed: f64| {
        let mut transform = new_transform();
        transform.resize(Size::new(1000, 1000)).unwrap();
        transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(8.0), ..Default::default() });
        let start = transform.view().center(WrapMode::Unwrapped);
        transform.start_momentum(&panning(sc(speed, 0.0), MomentumDecay::PAN), &AnimationOptions::default());
        transform.update_transitions(None);
        transform.view().center(WrapMode::Unwrapped).longitude() - start.longitude()
    };
    assert_near(glide(20_000.0), glide(10_000.0), 1e-9);
    assert!(glide(10_000.0).abs() > glide(5_000.0).abs());
}

// A curve pans along the release, its distance on screen.
#[test]
fn a_curve_pans_along_the_release() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(10.0, 20.0)), zoom: Some(8.0), ..Default::default() });
    let center = transform.view().center(WrapMode::Wrapped);
    let momentum = panning(sc(300.0, -400.0), even(500.0, Duration::from_secs(1)));
    let start = start_clock(&mut transform);
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(point.x, 500.0 + 150.0, 1e-6);
    assert_near(point.y, 500.0 - 200.0, 1e-6);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(point.x, 500.0 + 300.0, 1e-6);
    assert_near(point.y, 500.0 - 400.0, 1e-6);
    assert!(!transform.in_transition());
}

/// A transform 1000 px square at zoom 8, for the outcome tests.
fn outcome_transform() -> Transform {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(8.0), ..Default::default() });
    transform
}

fn outcome(transform: &Transform, ended: &EndLog) -> Option<TransitionOutcome> {
    ended.get(transform).map(|end| end.outcome)
}

#[test]
fn an_ease_that_runs_to_its_end_completes() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..animation },
    );
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    assert_eq!(outcome(&transform, &ended), None);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
    assert_eq!(ended.get(&transform).unwrap().velocity, CameraVelocity::default());
}

#[test]
fn cancelling_reports_cancelled_and_by_id_only_the_matching_commands() {
    let mut transform = outcome_transform();
    let (zoom, zoom_ended) = record_end();
    let (bearing, bearing_ended) = record_end();
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..zoom },
    );
    transform.ease_to(
        &CameraOptions { bearing: Some(90.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..bearing },
    );
    assert!(transform.cancel_transitions_with_id(zoom_ended.transition_id));
    assert_eq!(outcome(&transform, &zoom_ended), Some(TransitionOutcome::Cancelled));
    assert_eq!(outcome(&transform, &bearing_ended), None);
    transform.cancel_transitions();
    assert_eq!(outcome(&transform, &bearing_ended), Some(TransitionOutcome::Cancelled));
}

#[test]
fn a_command_whose_fields_a_later_one_takes_is_replaced() {
    let mut transform = outcome_transform();
    let (first, first_ended) = record_end();
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..first },
    );
    transform.ease_to(
        &CameraOptions { zoom: Some(6.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_secs(1)),
    );
    assert_eq!(outcome(&transform, &first_ended), Some(TransitionOutcome::Replaced));
}

#[test]
fn a_command_that_keeps_some_fields_completes_or_is_cancelled() {
    // Losing the zoom leaves the bearing animating to its end: completed.
    let mut transform = outcome_transform();
    let (both, ended) = record_end();
    let start = start_clock(&mut transform);
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), bearing: Some(90.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..both },
    );
    transform.jump_to(&CameraOptions { zoom: Some(6.0), ..Default::default() });
    assert_eq!(outcome(&transform, &ended), None);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));

    // Losing the zoom, then a cancellation of the bearing: cancelled.
    let mut transform = outcome_transform();
    let (both, ended) = record_end();
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), bearing: Some(90.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..both },
    );
    transform.jump_to(&CameraOptions { zoom: Some(6.0), ..Default::default() });
    transform.cancel_transitions();
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Cancelled));
}

#[test]
fn a_command_that_fails_before_it_moves_is_cancelled() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    transform.ease_to(&CameraOptions { zoom: Some(f64::NAN), ..Default::default() }, &animation);
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Cancelled));
    assert_near(transform.view().zoom(), 8.0, 1e-9);
}

#[test]
fn closing_the_transform_closes_its_commands() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..animation },
    );
    transform.close();
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Closed));
    assert!(!transform.in_transition());
}

fn change(mode: CameraChangeMode, source: CameraChangeSource) -> CameraChange {
    CameraChange { mode, source }
}

#[test]
fn camera_changes_report_whether_a_command_or_a_momentum_made_them() {
    use CameraChangeMode::{Animated, Immediate};
    use CameraChangeSource::{Command, Momentum};
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(8.0), ..Default::default() });
    take_changes(&mut transform);

    // A jump starts and ends at once.
    transform.jump_to(&CameraOptions { zoom: Some(9.0), ..Default::default() });
    assert_eq!(take_changes(&mut transform), [('w', change(Immediate, Command)), ('d', change(Immediate, Command))]);

    // An ease and a momentum run together; each change that ends one
    // reports that one's source.
    let ease_start = start_clock(&mut transform);
    transform.ease_to(
        &CameraOptions { zoom: Some(10.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_secs(1)),
    );
    transform.start_momentum(
        // A 1.4 s glide, which outlasts the ease.
        &panning(sc(10_000.0, 0.0), MomentumDecay::PAN),
        &AnimationOptions::default(),
    );
    assert_eq!(take_changes(&mut transform), [('w', change(Animated, Command)), ('w', change(Animated, Momentum))]);
    transform.update_transitions(Some(ease_start + Duration::from_secs(1)));
    assert_eq!(take_changes(&mut transform), [('d', change(Animated, Command))]);
    transform.update_transitions(None);
    assert_eq!(take_changes(&mut transform), [('d', change(Animated, Momentum))]);
}

fn delta(phase: GesturePhase) -> CameraDelta {
    CameraDelta { phase, ..CameraDelta::default() }
}

#[test]
fn a_gesture_reports_gesture_then_momentum_and_lasts_until_its_glide_ends() {
    use CameraChangeMode::{Animated, Immediate};
    use CameraChangeSource::{Gesture, Momentum};
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(8.0), ..Default::default() });
    take_changes(&mut transform);

    let pan = |phase| CameraDelta { offset: Some(sc(10.0, 0.0)), ..delta(phase) };
    transform.apply_camera_delta(&pan(GesturePhase::Begin)).unwrap();
    assert!(transform.is_gesture_in_progress());
    transform.apply_camera_delta(&pan(GesturePhase::Update)).unwrap();
    assert_eq!(
        take_changes(&mut transform),
        [
            ('w', change(Immediate, Gesture)),
            ('d', change(Immediate, Gesture)),
            ('w', change(Immediate, Gesture)),
            ('d', change(Immediate, Gesture)),
        ]
    );

    let (animation, ended) = record_end();
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(panning(sc(2000.0, 0.0), MomentumDecay::PAN)),
            animation,
            ..delta(GesturePhase::End)
        })
        .unwrap();
    assert_eq!(take_changes(&mut transform), [('w', change(Animated, Momentum))]);
    // The gesture's input ended, but it lasts until its momentum does.
    assert!(transform.is_gesture_in_progress());
    transform.update_transitions(None);
    assert_eq!(take_changes(&mut transform), [('d', change(Animated, Momentum))]);
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
    assert!(!transform.is_gesture_in_progress());
}

#[test]
fn a_new_gesture_catches_the_last_ones_glide_and_keeps_the_gesture_going() {
    let mut transform = outcome_transform();
    let start = start_clock(&mut transform);
    transform.apply_camera_delta(&CameraDelta { offset: Some(sc(10.0, 0.0)), ..delta(GesturePhase::Begin) }).unwrap();
    let (animation, ended) = record_end();
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(panning(sc(2000.0, 0.0), MomentumDecay::PAN)),
            animation,
            ..delta(GesturePhase::End)
        })
        .unwrap();
    transform.update_transitions(Some(start + Duration::from_millis(100)));

    // A press mid-glide: the glide is cancelled, with its speed, and the new
    // gesture is in progress.
    transform.apply_camera_delta(&CameraDelta { offset: Some(sc(1.0, 0.0)), ..delta(GesturePhase::Begin) }).unwrap();
    let end = ended.get(&transform).expect("the glide ended");
    assert_eq!(end.outcome, TransitionOutcome::Cancelled);
    assert!(end.velocity.pan.x > 500.0 && end.velocity.pan.x < 2000.0, "{:?}", end.velocity);
    assert!(transform.is_gesture_in_progress());
    assert!(!transform.in_transition());

    // An earlier gesture's momentum can't end a later gesture: start a glide
    // for the new one, then begin a third gesture with Update, which keeps
    // the glide; the glide's end leaves the third gesture in progress.
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(panning(sc(2000.0, 0.0), MomentumDecay::PAN)),
            ..delta(GesturePhase::End)
        })
        .unwrap();
    transform.apply_camera_delta(&CameraDelta { scale: Some(1.01), ..delta(GesturePhase::Update) }).unwrap();
    assert!(transform.in_transition());
    transform.update_transitions(None);
    assert!(!transform.in_transition());
    assert!(transform.is_gesture_in_progress());
    transform.apply_camera_delta(&delta(GesturePhase::End)).unwrap();
    assert!(!transform.is_gesture_in_progress());
}

#[test]
fn only_the_parts_a_gesture_changed_settle() {
    let mut snaps = CameraMomentum::default();
    snaps.zoom.snap = Some(MomentumSnap::ZOOM);
    snaps.bearing.snap = Some(MomentumSnap { duration: Duration::ZERO, ..MomentumSnap::BEARING });
    snaps.pitch.snap = Some(MomentumSnap::PITCH);
    let settled = |gesture: CameraDelta| {
        let mut transform = outcome_transform();
        transform.jump_to(&CameraOptions {
            zoom: Some(8.3),
            bearing: Some(3.0),
            pitch: Some(4.0),
            ..Default::default()
        });
        transform.apply_camera_delta(&CameraDelta { phase: GesturePhase::Begin, ..gesture }).unwrap();
        transform.apply_camera_delta(&CameraDelta { momentum: Some(snaps), ..delta(GesturePhase::End) }).unwrap();
        transform.update_transitions(None);
        (transform.view().zoom(), bearing_degrees(&transform), pitch_degrees(&transform))
    };

    // A pan changes none of them: nothing settles.
    let (zoom, bearing, pitch) = settled(CameraDelta { offset: Some(sc(10.0, 0.0)), ..CameraDelta::default() });
    assert_near(zoom, 8.3, 1e-9);
    assert_near(bearing, 3.0, 1e-9);
    assert_near(pitch, 4.0, 1e-9);

    // A turn settles the bearing alone.
    let (zoom, bearing, pitch) = settled(CameraDelta { bearing: Some(1.0), ..CameraDelta::default() });
    assert_near(zoom, 8.3, 1e-9);
    assert_near(bearing, 0.0, 1e-9);
    assert_near(pitch, 4.0, 1e-9);
}

#[test]
fn an_animated_snap_settles_without_a_clock() {
    let mut transform = outcome_transform();
    transform.jump_to(&CameraOptions { bearing: Some(3.0), ..Default::default() });
    transform.apply_camera_delta(&CameraDelta { bearing: Some(1.0), ..delta(GesturePhase::Begin) }).unwrap();
    let mut momentum = CameraMomentum::default();
    momentum.bearing.snap = Some(MomentumSnap::BEARING);
    transform.apply_camera_delta(&CameraDelta { momentum: Some(momentum), ..delta(GesturePhase::End) }).unwrap();
    transform.update_transitions(None);
    assert_near(bearing_degrees(&transform), 0.0, 1e-9);
    assert!(!transform.in_transition());
}

#[test]
fn a_delta_pans_and_zooms_around_its_anchor_and_ends_once() {
    let mut transform = outcome_transform();
    let anchor = sc(800.0, 300.0);
    // The pan applies first and brings this ground under the anchor, which
    // the zoom and turn then hold in place.
    let ground =
        transform.screen_coordinate_to_lat_lng(&sc(anchor.x + 50.0, anchor.y - 20.0), WrapMode::Unwrapped).unwrap();
    let (animation, ended) = record_end();
    transform
        .apply_camera_delta(&CameraDelta {
            offset: Some(sc(-50.0, 20.0)),
            scale: Some(2.0),
            bearing: Some(90.0),
            anchor: Some(anchor),
            animation,
            ..CameraDelta::default()
        })
        .unwrap();
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
    assert_near(transform.view().zoom(), 9.0, 1e-9);
    assert_near(bearing_degrees(&transform), 90.0, 1e-9);
    let under = transform.lat_lng_to_screen_coordinate(&ground);
    assert_near(under.x, anchor.x, 1e-6);
    assert_near(under.y, anchor.y, 1e-6);

    // Animated, both commands end the delta's one command together.
    let (animation, ended) = record_end();
    let start = start_clock(&mut transform);
    transform
        .apply_camera_delta(&CameraDelta {
            offset: Some(sc(10.0, 0.0)),
            scale: Some(2.0),
            animation: AnimationOptions { duration: Some(Duration::from_secs(1)), ..animation },
            ..CameraDelta::default()
        })
        .unwrap();
    transform.update_transitions(Some(start + Duration::from_millis(500)));
    assert_eq!(outcome(&transform, &ended), None);
    transform.update_transitions(Some(start + Duration::from_secs(1)));
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
}

#[test]
fn a_turn_around_an_anchor_keeps_the_ground_under_it() {
    let mut transform = outcome_transform();
    let anchor = sc(700.0, 200.0);
    let ground = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Unwrapped).unwrap();
    transform
        .apply_camera_delta(&CameraDelta { bearing: Some(90.0), anchor: Some(anchor), ..CameraDelta::default() })
        .unwrap();
    assert_near(bearing_degrees(&transform), 90.0, 1e-9);
    let under = transform.lat_lng_to_screen_coordinate(&ground);
    assert_near(under.x, anchor.x, 1e-6);
    assert_near(under.y, anchor.y, 1e-6);
}

// Port-only: a drag through apply_camera_delta holds the point its anchor
// names, as move_by_with_anchor does. On the globe the planet turns to keep
// the grabbed location under the cursor, which a drag around the view center
// doesn't; on a flat map seen from straight above at sea level every point
// moves alike, so the anchor changes nothing.
#[test]
fn a_drag_delta_holds_its_anchor() {
    let offset = sc(30.0, 12.0);
    let cursor = sc(330.0, 200.0);
    let drag = |transform: &mut Transform, anchor: Option<ScreenCoordinate>| {
        transform.apply_camera_delta(&CameraDelta { offset: Some(offset), anchor, ..CameraDelta::default() }).unwrap();
        transform.view().center(WrapMode::Unwrapped)
    };
    let globe = || {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(30.0, 20.0), 1.0);
        transform
    };

    let mut held = globe();
    let through_the_delta = drag(&mut held, Some(cursor));
    let mut direct = globe();
    direct.move_by_with_anchor(&offset, Some(cursor), &AnimationOptions::default()).unwrap();
    assert_eq!(through_the_delta, direct.view().center(WrapMode::Unwrapped));
    assert_eq!(held.view().zoom(), direct.view().zoom());
    let around_the_center = drag(&mut globe(), None);
    assert!(
        (through_the_delta.latitude() - around_the_center.latitude()).abs() > 0.1
            || (through_the_delta.longitude() - around_the_center.longitude()).abs() > 0.1,
        "{through_the_delta:?} {around_the_center:?}"
    );

    let flat = || {
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&CameraOptions { center: Some(ll(30.0, 20.0)), zoom: Some(3.0), ..Default::default() });
        transform
    };
    let (anchored, unanchored) = (drag(&mut flat(), Some(cursor)), drag(&mut flat(), None));
    assert_near(anchored.latitude(), unanchored.latitude(), 1e-9);
    assert_near(anchored.longitude(), unanchored.longitude(), 1e-9);
}

/// A 800 by 600 flat map at (30, 20), zoom 10, pitched 55 degrees and turned
/// 20, with its center `altitude` meters up, its clock started.
fn pitched_flat_map(altitude: f64) -> (Transform, Instant) {
    let mut transform = new_transform();
    transform.resize(Size::new(800, 600)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(30.0, 20.0)),
        zoom: Some(10.0),
        pitch: Some(55.0),
        bearing: Some(20.0),
        center_altitude: Some(altitude),
        ..Default::default()
    });
    let start = start_clock(&mut transform);
    (transform, start)
}

/// How far, in pixels, the flat map's ground at `location` draws from
/// `point` (y down): the flat map's ground is the plane at sea level.
fn off_by(transform: &Transform, location: &LatLng, point: ScreenCoordinate) -> f64 {
    let drawn = transform.lat_lng_to_screen_coordinate(location);
    (drawn.x - point.x).hypot(drawn.y - point.y)
}

/// The flat map's ground under `point` (y down).
fn flat_ground_under(transform: &Transform, point: ScreenCoordinate) -> LatLng {
    transform.screen_coordinate_to_lat_lng(&point, WrapMode::Unwrapped).unwrap()
}

/// Where the transform's projection matrix draws `lat_lng`, `height` meters
/// above sea level (y down).
fn drawn_at(transform: &Transform, lat_lng: &LatLng, height: f64) -> ScreenCoordinate {
    let view = transform.view();
    let point = Projection::project(lat_lng, view.scale());
    let clip = view.projection_matrix() * DVec4::new(point.x, point.y, height, 1.0);
    let size = view.size();
    sc(
        f64::midpoint(clip.x / clip.w, 1.0) * f64::from(size.width),
        (1.0 - clip.y / clip.w) / 2.0 * f64::from(size.height),
    )
}

/// Where the sky starts on the screen of a flat map that isn't rolled (y
/// down): above it, a line of sight doesn't descend to the ground.
fn horizon_row(transform: &Transform) -> f64 {
    let view = transform.view();
    view.padding().center_in(view.size()).y - view.mercator_horizon()
}

// Within about a pixel: the eye can't tell a grabbed point from the pointer
// at that distance.
const ABOUT_A_PIXEL: f64 = 1.0;

#[test]
fn drawn_at_sea_level_is_where_the_transform_draws_a_location() {
    let (transform, _) = pitched_flat_map(800.0);
    let location = flat_ground_under(&transform, sc(600.0, 520.0));
    let (drawn, expected) = (drawn_at(&transform, &location, 0.0), transform.lat_lng_to_screen_coordinate(&location));
    assert_near(drawn.x, expected.x, 1e-6);
    assert_near(drawn.y, expected.y, 1e-6);
}

/// On a pitched flat map, a drag keeps the ground it grabbed under the
/// pointer, wherever on the screen the pointer is, and its center 800 m up
/// moves it no differently: a drag solved at the view's center slid the
/// ground under a pointer near the bottom edge by tens of pixels a step.
#[test]
fn a_drag_on_a_pitched_flat_map_keeps_the_grabbed_ground_under_the_pointer() {
    for altitude in [0.0, 800.0] {
        let (mut transform, _) = pitched_flat_map(altitude);
        let mut pointer = sc(600.0, 520.0);
        let grabbed = flat_ground_under(&transform, pointer);
        for step in 0..20 {
            let offset = sc(-6.0, -12.0);
            pointer = sc(pointer.x + offset.x, pointer.y + offset.y);
            let phase = if step == 0 { GesturePhase::Begin } else { GesturePhase::Update };
            transform
                .apply_camera_delta(&CameraDelta {
                    offset: Some(offset),
                    anchor: Some(pointer),
                    phase,
                    ..CameraDelta::default()
                })
                .unwrap();
            let off = off_by(&transform, &grabbed, pointer);
            assert!(off < ABOUT_A_PIXEL, "{altitude} m, step {step}: the grabbed ground is {off} px off");
        }
        assert_eq!(altitude, transform.view().center_altitude(), "the drag keeps the altitude");
    }
}

/// The first pan of a pitched flat map whose center stands 800 m up moves
/// the ground at the view's center by the pan, as at sea level: a pan solved
/// at sea level jumped by the altitude times the tangent of the pitch.
#[test]
fn the_first_pan_of_a_raised_flat_map_does_not_jump() {
    let (mut transform, _) = pitched_flat_map(800.0);
    let center = sc(400.0, 300.0);
    let offset = sc(0.0, 2.0);
    let moving = transform
        .screen_coordinate_to_lat_lng_at(&sc(center.x - offset.x, center.y - offset.y), 800.0, WrapMode::Unwrapped)
        .unwrap();
    transform.move_by(&offset, &AnimationOptions::default()).unwrap();
    let drawn = drawn_at(&transform, &moving, 800.0);
    let off = (drawn.x - center.x).hypot(drawn.y - center.y);
    assert!(off < ABOUT_A_PIXEL, "the ground at the center is {off} px off");
}

/// On a pitched flat map 800 m up, a pinch's zoom and turn, an anchored
/// ease, and the momentum that carries a pinch on hold the ground under
/// their anchors, at sea level, within about a pixel at each step and frame.
#[test]
fn anchored_moves_on_a_raised_pitched_flat_map_hold_their_ground() {
    let anchor = sc(600.0, 450.0);
    let (mut transform, start) = pitched_flat_map(800.0);
    let grabbed = flat_ground_under(&transform, anchor);
    for (step, delta) in [
        CameraDelta { scale: Some(1.3), bearing: Some(-12.0), phase: GesturePhase::Begin, ..CameraDelta::default() },
        CameraDelta { scale: Some(0.8), pitch: Some(5.0), phase: GesturePhase::Update, ..CameraDelta::default() },
    ]
    .into_iter()
    .enumerate()
    {
        transform.apply_camera_delta(&CameraDelta { anchor: Some(anchor), ..delta }).unwrap();
        let off = off_by(&transform, &grabbed, anchor);
        assert!(off < ABOUT_A_PIXEL, "pinch step {step}: the ground is {off} px off");
    }

    let mut momentum = CameraMomentum::default();
    momentum.zoom.glide = Some(glide(1.5, MomentumDecay::DEFAULT));
    momentum.zoom.anchor = Some(anchor);
    momentum.bearing.glide = Some(glide(40.0, MomentumDecay::DEFAULT));
    momentum.bearing.anchor = Some(anchor);
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(momentum),
            phase: GesturePhase::End,
            ..CameraDelta::default()
        })
        .unwrap();
    for ms in (16..2000).step_by(16) {
        transform.update_transitions(Some(start + Duration::from_millis(ms)));
        let off = off_by(&transform, &grabbed, anchor);
        assert!(off < ABOUT_A_PIXEL, "{ms} ms of momentum: the ground is {off} px off");
    }
    assert!(!transform.in_transition());

    let (mut transform, start) = pitched_flat_map(800.0);
    let grabbed = flat_ground_under(&transform, anchor);
    transform.ease_to(
        &CameraOptions {
            zoom: Some(11.5),
            bearing: Some(-30.0),
            pitch: Some(40.0),
            center_altitude: Some(1000.0),
            anchor: Some(anchor),
            ..Default::default()
        },
        &AnimationOptions { duration: Some(Duration::from_millis(500)), ..Default::default() },
    );
    for ms in (16..=512).step_by(16) {
        transform.update_transitions(Some(start + Duration::from_millis(ms)));
        let off = off_by(&transform, &grabbed, anchor);
        assert!(off < ABOUT_A_PIXEL, "{ms} ms of the ease: the ground is {off} px off");
    }
    let camera = transform.camera_options(None);
    assert_eq!((Some(11.5), Some(1000.0)), (camera.zoom, camera.center_altitude), "the ease ends where it asked");
}

/// An 800 by 600 map under `projection` at (45, 7), zoom 13, pitched 60
/// degrees, with its center `altitude` meters up.
fn raised_at_zoom_13(projection: &str, altitude: f64) -> Transform {
    let mut transform = new_transform();
    transform.resize(Size::new(800, 600)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name(projection));
    transform.jump_to(&CameraOptions {
        center: Some(ll(45.0, 7.0)),
        zoom: Some(13.0),
        pitch: Some(60.0),
        center_altitude: Some(altitude),
        ..Default::default()
    });
    transform
}

/// A pan without an anchor on a pitched globe whose center stands 3000 m up
/// moves the center as far down the screen as Mercator's pan at the same
/// camera does: solved on the sea-level sphere, it moved it more than twice
/// as far.
#[test]
fn a_pan_of_a_raised_globe_moves_the_center_as_far_as_on_mercator() {
    let pan = |projection: &str| {
        let mut transform = raised_at_zoom_13(projection, 3000.0);
        let center = transform.view().center(WrapMode::Unwrapped);
        transform.move_by(&sc(0.0, 50.0), &AnimationOptions::default()).unwrap();
        let view = transform.view();
        flip_y(view, &view.lat_lng_to_screen_coordinate_at(&center, 3000.0))
    };
    let (globe, mercator) = (pan("vertical-perspective"), pan("mercator"));
    let off = (globe.x - mercator.x).hypot(globe.y - mercator.y);
    assert!(off < ABOUT_A_PIXEL, "the globe's center lands at {globe:?}, Mercator's at {mercator:?}");
}

/// On a pitched globe, a drag keeps the ground it grabbed, at sea level,
/// under the pointer, and its center 3000 m up moves it no differently.
#[test]
fn a_drag_on_a_raised_globe_keeps_the_grabbed_ground_under_the_pointer() {
    for altitude in [0.0, 3000.0] {
        let mut transform = raised_at_zoom_13("vertical-perspective", altitude);
        let mut pointer = sc(600.0, 520.0);
        let grabbed = flat_ground_under(&transform, pointer);
        for step in 0..20 {
            let offset = sc(-6.0, -12.0);
            pointer = sc(pointer.x + offset.x, pointer.y + offset.y);
            let phase = if step == 0 { GesturePhase::Begin } else { GesturePhase::Update };
            transform
                .apply_camera_delta(&CameraDelta {
                    offset: Some(offset),
                    anchor: Some(pointer),
                    phase,
                    ..CameraDelta::default()
                })
                .unwrap();
            let off = off_by(&transform, &grabbed, pointer);
            assert!(off < ABOUT_A_PIXEL, "{altitude} m, step {step}: the grabbed ground is {off} px off");
        }
    }
}

/// Where the flat map's old rule and its plane agree, on a map seen from
/// straight above at sea level, a gesture moves the camera as it did: a drag
/// pans by its offset at the view's center, and a pinch's zoom and turn hold
/// the location under the anchor, as the state's own conversions put them.
#[test]
fn from_straight_above_at_sea_level_flat_gestures_move_as_before() {
    let flipped = |point: ScreenCoordinate| sc(point.x, 600.0 - point.y);
    let mut transform = new_transform();
    transform.resize(Size::new(800, 600)).unwrap();
    transform.jump_to(&CameraOptions {
        center: Some(ll(30.0, 20.0)),
        zoom: Some(10.0),
        bearing: Some(20.0),
        ..Default::default()
    });
    let anchor = sc(500.0, 400.0);

    let mut expected = transform.state().clone();
    let offset = sc(12.0, -30.0);
    transform
        .apply_camera_delta(&CameraDelta {
            offset: Some(offset),
            anchor: Some(anchor),
            phase: GesturePhase::Begin,
            ..CameraDelta::default()
        })
        .unwrap();
    let center = expected
        .view()
        .screen_coordinate_to_lat_lng(&flipped(sc(400.0 - offset.x, 300.0 - offset.y)), WrapMode::Unwrapped)
        .unwrap();
    expected.set_lat_lng_zoom(&center, 10.0);
    let (actual, expected_center) =
        (transform.view().center(WrapMode::Unwrapped), expected.view().center(WrapMode::Unwrapped));
    assert_near(expected_center.latitude(), actual.latitude(), 1e-9);
    assert_near(expected_center.longitude(), actual.longitude(), 1e-9);

    let mut expected = transform.state().clone();
    let before = transform.camera_options(None);
    let location = expected.view().screen_coordinate_to_lat_lng(&flipped(anchor), WrapMode::Unwrapped).unwrap();
    transform
        .apply_camera_delta(&CameraDelta {
            scale: Some(1.3),
            bearing: Some(-12.0),
            anchor: Some(anchor),
            phase: GesturePhase::Update,
            ..CameraDelta::default()
        })
        .unwrap();
    expected.set_angle(deg2rad(-(before.bearing.unwrap() - 12.0)));
    expected.set_lat_lng_zoom(&before.center.unwrap(), before.zoom.unwrap() + 1.3f64.log2());
    expected.move_lat_lng(&location, &flipped(anchor)).unwrap();
    let (expected, actual) = (expected.camera_options(None), transform.camera_options(None));
    assert_eq!((expected.zoom, expected.bearing), (actual.zoom, actual.bearing), "the pinch");
    let (expected, actual) = (expected.center.unwrap(), actual.center.unwrap());
    assert_near(expected.latitude(), actual.latitude(), 1e-9);
    assert_near(expected.longitude(), actual.longitude(), 1e-9);
}

/// A tilt around a point just below the horizon of a flat map, which
/// carries it above the horizon, turns the camera around the view's center
/// instead, as a tilt without an anchor does: a gesture's step, and the
/// frames of a command once they carry it there.
#[test]
fn a_tilt_that_carries_its_anchor_into_the_sky_turns_around_the_center() {
    let flat_map = |pitch: f64| {
        let mut transform = new_transform();
        transform.set_max_pitch(85.0);
        transform.resize(Size::new(800, 600)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(ll(30.0, 20.0)),
            zoom: Some(12.0),
            pitch: Some(pitch),
            ..Default::default()
        });
        transform
    };
    let tilt = |anchored: bool| {
        let mut transform = flat_map(70.0);
        let anchor = sc(330.0, horizon_row(&transform) + 10.0);
        let delta = CameraDelta {
            pitch: Some(10.0),
            anchor: anchored.then_some(anchor),
            phase: GesturePhase::Begin,
            ..Default::default()
        };
        transform.apply_camera_delta(&delta).unwrap();
        assert!(anchor.y < horizon_row(&transform), "the tilt carries the anchor into the sky");
        transform.camera_options(None)
    };
    assert_eq!(tilt(false), tilt(true), "a gesture's tilt");

    let mut transform = flat_map(60.0);
    let start = start_clock(&mut transform);
    let anchor = sc(330.0, horizon_row(&transform) + 150.0);
    let grabbed = flat_ground_under(&transform, anchor);
    transform.ease_to(
        &CameraOptions { pitch: Some(85.0), anchor: Some(anchor), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..Default::default() },
    );
    let (mut held, mut skyward) = (0, 0);
    let mut center = transform.view().center(WrapMode::Unwrapped);
    for ms in (16..=1008).step_by(16) {
        transform.update_transitions(Some(start + Duration::from_millis(ms)));
        let now = transform.view().center(WrapMode::Unwrapped);
        if skyward > 0 || anchor.y < horizon_row(&transform) {
            skyward += 1;
            // Recomputed each frame, so equal only to rounding.
            let moved = (now.latitude() - center.latitude()).abs().max((now.longitude() - center.longitude()).abs());
            assert!(moved < 1e-9, "{ms} ms: the anchor reached the sky, and the center moved {moved}°");
        } else {
            held += 1;
            let off = off_by(&transform, &grabbed, anchor);
            assert!(off < 1e-6, "{ms} ms: the ground is {off} px off");
        }
        center = now;
    }
    assert!(held > 5 && skyward > 5, "{held} frames held the anchor, and {skyward} didn't");
}

/// Under a roll, where the horizon isn't a row of the screen, a turn around
/// a point whose line of sight doesn't descend to the ground turns around
/// the view's center, as a turn without an anchor does, and a turn around a
/// point on the ground holds that ground under it.
#[test]
fn a_turn_under_a_roll_holds_only_a_point_on_the_ground() {
    let rolled = || {
        let mut transform = new_transform();
        transform.set_max_pitch(85.0);
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(ll(30.0, 20.0)),
            zoom: Some(12.0),
            pitch: Some(80.0),
            roll: Some(90.0),
            ..Default::default()
        });
        transform
    };
    let turn = |anchor: Option<ScreenCoordinate>| {
        let mut transform = rolled();
        let delta = CameraDelta { bearing: Some(15.0), anchor, phase: GesturePhase::Begin, ..Default::default() };
        transform.apply_camera_delta(&delta).unwrap();
        transform
    };
    let transform = rolled();
    let view = transform.view();
    let descends = |point: ScreenCoordinate| {
        let (near, far) = view.line_of_sight(&sc(point.x, 512.0 - point.y)).unwrap();
        far.z < near.z
    };
    // The roll puts the sky on one side of the screen and the ground on the
    // other.
    let (left, right) = (sc(20.0, 480.0), sc(492.0, 480.0));
    let (sky, ground) = if descends(left) { (right, left) } else { (left, right) };
    assert!(!descends(sky) && descends(ground), "the roll turns the horizon upright");
    assert!(sky.y > horizon_row(&transform), "below the row of the unrolled horizon");

    assert_eq!(turn(None).camera_options(None), turn(Some(sky)).camera_options(None), "a turn around the sky");

    let grabbed = flat_ground_under(&transform, ground);
    let off = off_by(&turn(Some(ground)), &grabbed, ground);
    assert!(off < 0.01, "the ground moved {off} px from the anchor");
}

/// A drag whose pointer comes back below the horizon after a step above it
/// grabs no ground on its way back: it moves the map on the plane at the
/// center's altitude, as a drag without an anchor does, rather than solving
/// for a point in the sky.
#[test]
fn a_drag_back_from_the_sky_pans_on_the_center_plane() {
    let pan = |anchored: bool| {
        let mut transform = new_transform();
        transform.set_max_pitch(85.0);
        transform.resize(Size::new(800, 600)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(ll(30.0, 20.0)),
            zoom: Some(12.0),
            pitch: Some(75.0),
            ..Default::default()
        });
        let below = sc(330.0, horizon_row(&transform) + 15.0);
        let offset = sc(0.0, -30.0);
        // Up across the horizon, then back.
        let steps = [(offset, sc(below.x, below.y + offset.y)), (sc(0.0, -offset.y), below)];
        for (step, (offset, pointer)) in steps.into_iter().enumerate() {
            let phase = if step == 0 { GesturePhase::Begin } else { GesturePhase::Update };
            let delta =
                CameraDelta { offset: Some(offset), anchor: anchored.then_some(pointer), phase, ..Default::default() };
            transform.apply_camera_delta(&delta).unwrap();
        }
        transform.camera_options(None)
    };
    assert_eq!(pan(false), pan(true));
}

#[test]
fn camera_deltas_are_validated_as_maplibre_native_ffi_validates_them() {
    let mut transform = outcome_transform();
    let mut check = |delta: CameraDelta| transform.apply_camera_delta(&delta);
    assert_eq!(
        check(CameraDelta { offset: Some(sc(f64::NAN, 0.0)), ..CameraDelta::default() }),
        Err(CameraError::NotFinite)
    );
    assert_eq!(check(CameraDelta { scale: Some(0.0), ..CameraDelta::default() }), Err(CameraError::InvalidScale));
    assert_eq!(
        check(CameraDelta { bearing: Some(f64::INFINITY), ..CameraDelta::default() }),
        Err(CameraError::NotFinite)
    );
    // An anchor with only an offset is the point a drag holds (the globe's
    // `moveBy(offset, anchor)`); one with nothing to hold is rejected.
    assert_eq!(
        check(CameraDelta { anchor: Some(sc(1.0, 1.0)), ..CameraDelta::default() }),
        Err(CameraError::AnchorWithoutChange)
    );
    assert_eq!(
        check(CameraDelta { momentum: Some(CameraMomentum::default()), ..delta(GesturePhase::Update) }),
        Err(CameraError::MomentumOutsideEnd)
    );
    assert_eq!(
        check(CameraDelta {
            momentum: Some(CameraMomentum::default()),
            animation: AnimationOptions::new(Duration::from_secs(1)),
            ..delta(GesturePhase::End)
        }),
        Err(CameraError::MomentumWithAnimation)
    );
    assert_eq!(
        check(CameraDelta {
            offset: Some(sc(1.0, 0.0)),
            scale: Some(2.0),
            anchor: Some(sc(1.0, 1.0)),
            animation: AnimationOptions::new(Duration::from_secs(1)),
            ..CameraDelta::default()
        }),
        Err(CameraError::AnimatedAnchoredOffset)
    );
    // A rejected delta changes nothing; an empty one ends at once.
    assert_near(transform.view().zoom(), 8.0, 1e-9);
    let (animation, ended) = record_end();
    transform.apply_camera_delta(&CameraDelta { animation, ..CameraDelta::default() }).unwrap();
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Completed));
}

#[test]
fn an_update_with_a_phase_reports_a_gesture_and_begin_cancels_animations() {
    use CameraChangeSource::Gesture;
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let (animation, ended) = record_end();
    transform.ease_to(
        &CameraOptions { zoom: Some(6.0), ..Default::default() },
        &AnimationOptions { duration: Some(Duration::from_secs(1)), ..animation },
    );

    // Update keeps the running animation.
    transform.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { bearing: Some(30.0), ..Default::default() },
        animation: AnimationOptions::new(Duration::from_millis(300)),
        phase: GesturePhase::End,
    });
    assert_eq!(outcome(&transform, &ended), None);
    assert!(!transform.is_gesture_in_progress());
    assert_eq!(take_changes(&mut transform).last().map(|(_, c)| c.source), Some(Gesture));

    // Begin cancels it.
    transform.update_camera(&CameraUpdate {
        camera: CameraOptions { pitch: Some(10.0), ..Default::default() },
        phase: GesturePhase::Begin,
        ..CameraUpdate::default()
    });
    assert_eq!(outcome(&transform, &ended), Some(TransitionOutcome::Cancelled));
    assert!(transform.is_gesture_in_progress());
    assert_near(pitch_degrees(&transform), 10.0, 1e-9);
}

#[test]
fn with_a_frame_time_commands_start_at_the_latest_frame() {
    let mut transform = outcome_transform();
    let frame = Instant::now();
    transform.set_frame_time(Some(frame));
    // Made between frames, the ease starts at the latest frame's time, so
    // the next frame shows it one frame in.
    let linear = UnitBezier::new(0.0, 0.0, 1.0, 1.0);
    transform.ease_to(
        &CameraOptions { zoom: Some(9.0), ..Default::default() },
        &AnimationOptions {
            duration: Some(Duration::from_millis(160)),
            easing: Some(linear),
            ..AnimationOptions::default()
        },
    );
    transform.update_transitions(Some(frame + Duration::from_millis(16)));
    assert_near(transform.view().zoom(), 8.1, 1e-4);

    // A momentum starts there too: from a frame an hour ahead, it still
    // glides one frame after that frame.
    let frame = start_clock(&mut transform);
    transform.start_momentum(&panning(sc(1000.0, 0.0), MomentumDecay::PAN), &AnimationOptions::default());
    transform.update_transitions(Some(frame + Duration::from_millis(16)));
    assert!(transform.in_transition());
    transform.cancel_transitions();

    // Without one, commands start at Instant::now(): neither earlier nor at the
    // last frame's time, an hour ahead.
    transform.set_frame_time(None);
    let before = Instant::now();
    transform.ease_to(
        &CameraOptions { bearing: Some(10.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_secs(1)),
    );
    transform.update_transitions(Some(before));
    assert_near(bearing_degrees(&transform), 0.0, 1e-12);
    transform.update_transitions(Some(Instant::now() + Duration::from_secs(1)));
    assert_near(bearing_degrees(&transform), 10.0, 1e-9);
}

#[test]
fn a_stale_frame_time_doesnt_start_commands_in_the_past() {
    let mut transform = outcome_transform();
    // The host set a frame time a second ago and has drawn no frame since.
    transform.set_frame_time(Instant::now().checked_sub(Duration::from_secs(1)));
    assert_eq!(transform.frame_time(), None);
    let before = Instant::now();
    transform.ease_to(
        &CameraOptions { zoom: Some(9.0), ..Default::default() },
        &AnimationOptions::new(Duration::from_millis(300)),
    );
    // It starts no earlier than now, so it hasn't run its course by the next
    // frame.
    transform.update_transitions(Some(before + Duration::from_millis(16)));
    assert!(transform.in_transition());
    assert!(transform.view().zoom() < 9.0);

    // A fresh one is used, from a future frame too.
    let fresh = Instant::now() + Duration::from_millis(5);
    transform.set_frame_time(Some(fresh));
    assert_eq!(transform.frame_time(), Some(fresh));
}

#[test]
fn overshooting_mercator_ease_interpolates_center_and_zoom_past_target() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&CameraOptions { center: Some(ll(0.0, 0.0)), zoom: Some(4.0), ..Default::default() });

    let start = start_clock(&mut transform);
    let destination = ll(0.0, 10.0);
    let target_zoom = 6.0;
    let duration = Duration::from_secs(1);
    transform.ease_to(
        &CameraOptions { center: Some(destination), zoom: Some(target_zoom), ..Default::default() },
        &AnimationOptions {
            duration: Some(duration),
            easing: Some(UnitBezier::new(1.0 / 3.0, 0.0, 2.0 / 3.0, 2.0)),
            ..AnimationOptions::default()
        },
    );

    // At 75% elapsed time this curve evaluates above 1.0 (~1.266), so both
    // center and zoom must overshoot past their destinations.
    transform.update_transitions(Some(start + Duration::from_millis(750)));
    assert!(transform.in_transition());
    assert!(transform.view().center(WrapMode::Unwrapped).longitude() > destination.longitude() + 1.0);
    assert!(transform.view().zoom() > target_zoom + 0.2);

    // At completion (t == 1.0) the animation lands on the exact destination.
    transform.update_transitions(Some(start + duration));
    assert!(!transform.in_transition());
    assert_double_eq(destination.latitude(), transform.view().center(WrapMode::Wrapped).latitude());
    assert_double_eq(destination.longitude(), transform.view().center(WrapMode::Wrapped).longitude());
    assert_double_eq(target_zoom, transform.view().zoom());
}
