// Ports test/map/transform.test.cpp (TEST(Transform, *), TEST(Camera, *), the
// projection seam's TEST(MercatorProjection, *) and TEST(TransformState, *),
// and the globe's TEST(VerticalPerspectiveProjection, *),
// TEST(GlobeTransform, *) and TEST(TileProjector, *)).
//
// The C++ AnimationOptions callbacks capture the Transform by reference and
// assert on it. The Rust callbacks can't borrow the Transform that is calling
// them (`ease_to`/`update_transitions` hold it mutably), so they record that
// they ran (`CallbackLog`) and the test asserts on the Transform right after
// the call that ran them. Nothing changes the state between a callback and
// the return of that call: after `transition_frame_fn` the frame closure only
// calls `on_camera_is_changing`, after `transition_end_fn` the end
// closure only calls `on_camera_did_change`, and `update_transitions` only
// moves the closures back.

use std::cell::{Cell, RefCell};

use mln_map_types::projection_base::ProjectionBase;
use mln_map_types::util_camera::Camera as UtilCamera;
use mln_map_types::{MercatorProjection, TileProjector};
use mln_style_types::ProjectionType;
use mln_tile_id::{CanonicalTileID, UnwrappedTileID};
use mln_util::UnitBezier;
use mln_util::chrono::{milliseconds, seconds};
use mln_util::constants::{EXTENT, LATITUDE_MAX, LONGITUDE_MAX, MAX_ZOOM, TILE_SIZE_D};
use mln_util::mat4::{Mat4, matrix};
use mln_util::quaternion::Quaternion;
use mln_util::vectors::{Vec3, Vec4};

use super::*;

fn new_transform() -> Transform {
    Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default)
}

fn cam() -> CameraOptions {
    CameraOptions::default()
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

fn sc(x: f64, y: f64) -> ScreenCoordinate {
    ScreenCoordinate::new(x, y)
}

fn insets(top: f64, left: f64, bottom: f64, right: f64) -> EdgeInsets {
    EdgeInsets::new(top, left, bottom, right).unwrap()
}

fn deg2rad(deg: f64) -> f64 {
    mln_math::deg2rad(deg)
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

/// `EXPECT_THAT(arg, Vec3NearEquals1E5(vec))`.
#[track_caller]
fn assert_vec3_near_1e5(arg: &Vec3, vec: &Vec3) {
    let near = (vec[0] - arg[0]).abs() <= ABS_DOUBLE_ERROR
        && (vec[1] - arg[1]).abs() <= ABS_DOUBLE_ERROR
        && (vec[2] - arg[2]).abs() <= ABS_DOUBLE_ERROR;
    assert!(near, "Vec3NearEquals1E5: {arg:?} vs {vec:?}");
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

/// `std::hypot(x, y, z)`.
fn hypot3(x: f64, y: f64, z: f64) -> f64 {
    (x * x + y * y + z * z).sqrt()
}

/// `std::clamp(value, lo, hi)`: `value < lo ? lo : hi < value ? hi : value`.
fn std_clamp(value: f64, lo: f64, hi: f64) -> f64 {
    if value < lo {
        lo
    } else if hi < value {
        hi
    } else {
        value
    }
}

fn rotated_frame(quaternion: &Vec4) -> (Vec3, Vec3, Vec3) {
    let q = Quaternion::from_vec4(quaternion);
    (q.transform(&[1.0, 0.0, 0.0]), q.transform(&[0.0, -1.0, 0.0]), q.transform(&[0.0, 0.0, -1.0]))
}

/// Records the AnimationOptions callbacks (see the file comment).
struct CallbackLog {
    frames: Rc<RefCell<Vec<f64>>>,
    finishes: Rc<Cell<u32>>,
}

impl CallbackLog {
    fn install(options: &mut AnimationOptions) -> CallbackLog {
        let log = CallbackLog { frames: Rc::default(), finishes: Rc::default() };
        let frames = log.frames.clone();
        options.transition_frame_fn = Some(Rc::new(move |t: f64| frames.borrow_mut().push(t)));
        let finishes = log.finishes.clone();
        options.transition_end_fn = Some(Rc::new(move |_: &TransitionEnd| finishes.set(finishes.get() + 1)));
        log
    }

    /// The `t` arguments of the frame callbacks since the last call.
    fn take_frames(&self) -> Vec<f64> {
        std::mem::take(&mut *self.frames.borrow_mut())
    }

    /// Whether the finish callback ran since the last call.
    fn take_finish(&self) -> bool {
        self.finishes.replace(0) > 0
    }
}

// gtest: Transform.InvalidZoom
#[test]
fn invalid_zoom() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.get_zoom());

    transform.jump_to(&cam().with_zoom(Some(1.0)));

    assert_double_eq(1.0, transform.get_zoom());

    let invalid = f64::NAN;

    transform.jump_to(&cam().with_zoom(Some(invalid)));

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.get_zoom());

    transform.jump_to(&cam().with_center(Some(LatLng::default())).with_zoom(Some(invalid)));

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.get_zoom());

    let max_zoom = transform.get_state().get_max_zoom();
    transform.jump_to(&cam().with_zoom(Some(max_zoom + 0.1)));
    assert_double_eq(transform.get_zoom(), transform.get_state().get_max_zoom());

    // Executing flyTo with an empty size causes frameZoom to be NaN.
    let max_zoom = transform.get_state().get_max_zoom();
    transform.fly_to(
        &cam().with_center(Some(ll(LATITUDE_MAX, LONGITUDE_MAX))).with_zoom(Some(max_zoom)),
        &AnimationOptions::default(),
        false,
    );
    let now = transform.get_latest_transition_start() + transform.get_latest_transition_duration();
    transform.update_transitions(now);
    assert_double_eq(transform.get_zoom(), transform.get_state().get_max_zoom());

    // Executing flyTo with maximum zoom level to the same zoom level causes
    // frameZoom to be bigger than maximum zoom.
    transform.resize(Size::new(100, 100)).unwrap();
    let max_zoom = transform.get_state().get_max_zoom();
    transform.fly_to(
        &cam().with_center(Some(ll(LATITUDE_MAX, LONGITUDE_MAX))).with_zoom(Some(max_zoom)),
        &AnimationOptions::default(),
        false,
    );
    let now = transform.get_latest_transition_start() + transform.get_latest_transition_duration();
    transform.update_transitions(now);

    assert!(transform.get_state().valid());
    assert_double_eq(transform.get_state().get_max_zoom(), transform.get_zoom());
}

// gtest: Transform.InvalidBearing
#[test]
fn invalid_bearing() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.get_zoom());

    transform.jump_to(&cam().with_zoom(Some(1.0)).with_bearing(Some(2.0)));
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.get_zoom());
    assert_double_eq(deg2rad(-2.0), transform.get_bearing());

    let invalid = f64::NAN;

    transform.jump_to(&cam().with_bearing(Some(invalid)));
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(1.0, transform.get_zoom());
    assert_double_eq(deg2rad(-2.0), transform.get_bearing());
}

// gtest: Transform.IntegerZoom
#[test]
fn integer_zoom() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    let mut check_integer_zoom = |zoom_int: u8, zoom: f64| {
        transform.jump_to(&cam().with_zoom(Some(zoom)));
        assert_near(transform.get_zoom(), zoom, 1e-8);
        assert_eq!(transform.get_state().get_integer_zoom(), zoom_int);
        assert_near(transform.get_state().get_zoom_fraction(), zoom - zoom_int as f64, 1e-8);
    };

    for zoom_int in 0u8..20 {
        for percent in 0u32..100 {
            let zoom = zoom_int as f64 + (0.01 * percent as f64);
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
    transform.jump_to(&cam().with_center(Some(ll(38.0, -77.0))).with_zoom(Some(10.0)).with_pitch(Some(51.56620156)));

    // expected values are from maplibre-gl-js

    let loc = transform.get_lat_lng(WrapMode::Wrapped);
    assert_double_eq(-77.0, loc.longitude());
    assert_double_eq(38.0, loc.latitude());

    let loc = transform.get_state().screen_coordinate_to_lat_lng(&sc(0.0, 1000.0), WrapMode::Unwrapped).unwrap();
    assert_near(-77.59198961199148, loc.longitude(), 1e-6);
    assert_near(38.74661326302018, loc.latitude(), 1e-6);

    let loc = transform.get_state().screen_coordinate_to_lat_lng(&sc(1000.0, 0.0), WrapMode::Unwrapped).unwrap();
    assert_near(-76.75823239205641, loc.longitude(), 1e-6);
    assert_near(37.692872969426375, loc.latitude(), 1e-6);

    let point = transform.get_state().lat_lng_to_screen_coordinate(&ll(38.74661326302018, -77.59198961199148));
    assert_near(point.x, 0.0, 1e-5);
    assert_near(point.y, 1000.0, 1e-4);

    let point = transform.get_state().lat_lng_to_screen_coordinate(&ll(37.692872969426375, -76.75823239205641));
    assert_near(point.x, 1000.0, 1e-5);
    assert_near(point.y, 0.0, 1e-4);

    let mut p: Vec4 = [0.0; 4];
    let point =
        transform.get_state().lat_lng_to_screen_coordinate_with(&ll(37.692872969426375, -76.75823239205641), &mut p);
    assert_near(point.x, 1000.0, 1e-5);
    assert_near(point.y, 0.0, 1e-4);
    assert!(p[3] > 0.0);

    transform.jump_to(&cam().with_center(Some(ll(38.0, -77.0))).with_zoom(Some(18.0)).with_pitch(Some(51.56620156)));
    let _point =
        transform.get_state().lat_lng_to_screen_coordinate_with(&ll(7.692872969426375, -76.75823239205641), &mut p);
    assert!(p[3] < 0.0);
}

// gtest: Transform.UnwrappedLatLng
#[test]
fn unwrapped_lat_lng() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    // 0.9 rad ~ 51.56620156 deg
    transform.jump_to(&cam().with_center(Some(ll(38.0, -77.0))).with_zoom(Some(10.0)).with_pitch(Some(51.56620156)));

    let state = transform.get_state();

    let from_get_lat_lng = state.get_lat_lng(WrapMode::Unwrapped);
    assert_double_eq(from_get_lat_lng.latitude(), 38.0);
    assert_double_eq(from_get_lat_lng.longitude(), -77.0);

    let from_screen_coordinate = state.screen_coordinate_to_lat_lng(&sc(500.0, 500.0), WrapMode::Unwrapped).unwrap();
    assert_near(from_screen_coordinate.latitude(), 38.0, 1e-8);
    assert_near(from_screen_coordinate.longitude(), -77.0, 1e-8);

    let mut wrapped_rightwards = state
        .screen_coordinate_to_lat_lng(&state.lat_lng_to_screen_coordinate(&ll(38.0, 283.0)), WrapMode::Unwrapped)
        .unwrap();
    assert_near(wrapped_rightwards.latitude(), 38.0, 1e-8);
    assert_near(wrapped_rightwards.longitude(), 283.0, 1e-8);
    wrapped_rightwards.wrap();
    assert_near(wrapped_rightwards.longitude(), -77.0, 1e-8);

    let mut wrapped_leftwards = state
        .screen_coordinate_to_lat_lng(&state.lat_lng_to_screen_coordinate(&ll(38.0, -437.0)), WrapMode::Unwrapped)
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

    transform.jump_to(&cam().with_center(Some(ll(0.0, -10.0))).with_zoom(Some(4.0)));
    assert_near(-181.0, transform.get_state().lat_lng_near_center(&ll(0.0, 179.0)).longitude(), 1e-9);
    assert_near(170.0, transform.get_state().lat_lng_near_center(&ll(0.0, 170.0)).longitude(), 1e-9);

    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 181.0))));
    assert_near(181.0, transform.get_state().get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(179.0, transform.get_state().lat_lng_near_center(&ll(0.0, 179.0)).longitude(), 1e-9);
    assert_near(180.2, transform.get_state().lat_lng_near_center(&ll(0.0, -179.8)).longitude(), 1e-9);
    assert_near(350.0, transform.get_state().lat_lng_near_center(&ll(0.0, -10.0)).longitude(), 1e-9);
}

// gtest: Transform.ConstrainHeightOnly
#[test]
fn constrain_height_only() {
    let mut transform =
        Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(2, 2)).unwrap();

    transform.jump_to(&cam().with_center(Some(LatLngBounds::world().southwest())).with_zoom(Some(MAX_ZOOM)));
    assert_near(-LATITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-7);

    transform.jump_to(&cam().with_center(Some(LatLngBounds::world().northeast())));
    assert_near(LATITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-7);
}

// gtest: Transform.ConstrainWidthAndHeight
#[test]
fn constrain_width_and_height() {
    let mut transform =
        Transform::new(Rc::new(NullTransformObserver), ConstrainMode::WidthAndHeight, ViewportMode::Default);
    transform.resize(Size::new(2, 2)).unwrap();

    transform.jump_to(&cam().with_center(Some(LatLngBounds::world().southwest())).with_zoom(Some(MAX_ZOOM)));
    assert_near(-LATITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);

    transform.jump_to(&cam().with_center(Some(LatLngBounds::world().northeast())));
    assert_near(LATITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-7);
    assert_near(-LONGITUDE_MAX, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
}

// gtest: Transform.Anchor
#[test]
fn anchor() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    let lat_lng = ll(10.0, -100.0);
    let anchor_point = sc(150.0, 150.0);

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(10.0)));
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(10.0, transform.get_zoom());
    assert_double_eq(0.0, transform.get_bearing());

    let anchor_lat_lng =
        transform.get_state().screen_coordinate_to_lat_lng(&anchor_point, WrapMode::Unwrapped).unwrap();
    assert_ne!(lat_lng.latitude(), anchor_lat_lng.latitude());
    assert_ne!(lat_lng.longitude(), anchor_lat_lng.longitude());

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(3.0)));
    assert_double_eq(3.0, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(3.5)));
    assert_double_eq(3.5, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(5.5)).with_anchor(Some(anchor_point)));
    assert_double_eq(5.5, transform.get_zoom());
    assert_ne!(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(3.0)));
    assert_double_eq(3.0, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(5.0)));
    assert_double_eq(5.0, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(7.0)).with_anchor(Some(anchor_point)));
    assert_double_eq(7.0, transform.get_zoom());
    assert_ne!(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(2.0)));
    assert_double_eq(2.0, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(4.0)));
    assert_double_eq(4.0, transform.get_zoom());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_zoom(Some(8.0)).with_anchor(Some(anchor_point)));
    assert_double_eq(8.0, transform.get_zoom());
    assert_ne!(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_ne!(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(10.0)).with_bearing(Some(-45.0)));
    assert_double_eq(PI / 4.0, transform.get_bearing());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_bearing(Some(0.0)));
    assert_double_eq(0.0, transform.get_bearing());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_bearing(Some(45.0)).with_anchor(Some(anchor_point)));
    assert_double_eq(deg2rad(-45.0), transform.get_bearing());

    // Anchor coordinates are imprecise because we are converting from an integer pixel.
    assert_near(anchor_lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.5);
    assert_near(anchor_lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.5);

    transform.jump_to(&cam().with_center(Some(lat_lng)).with_zoom(Some(10.0)).with_pitch(Some(10.0)));
    assert_double_eq(deg2rad(10.0), transform.get_pitch());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_pitch(Some(15.0)));
    assert_double_eq(deg2rad(15.0), transform.get_pitch());
    assert_double_eq(lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());

    transform.jump_to(&cam().with_pitch(Some(20.0)).with_anchor(Some(anchor_point)));
    assert_double_eq(deg2rad(20.0), transform.get_pitch());

    // Anchor coordinates are imprecise because we are converting from an integer pixel.
    assert_near(anchor_lat_lng.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.5);
    assert_near(anchor_lat_lng.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.5);
}

// gtest: Transform.Padding
#[test]
fn padding() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    let non_padded_camera_options = cam().with_center(Some(ll(10.0, -100.0))).with_zoom(Some(10.0));
    transform.jump_to(&non_padded_camera_options);

    let true_center = transform.get_lat_lng(WrapMode::Wrapped);
    assert_double_eq(10.0, true_center.latitude());
    assert_double_eq(-100.0, true_center.longitude());
    assert_double_eq(10.0, transform.get_zoom());

    let screen_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 / 2.0), WrapMode::Wrapped).unwrap();
    let upper_half_center =
        transform.screen_coordinate_to_lat_lng(&sc(1000.0 / 2.0, 1000.0 * 0.25), WrapMode::Wrapped).unwrap();

    let padding = insets(1000.0 / 2.0, 0.0, 0.0, 0.0);
    // CameraOption center and zoom don't change when padding changes: center of
    // viewport remains the same as padding defines viwport center offset in rendering.
    let padded_options = cam().with_padding(Some(padding));
    transform.jump_to(&padded_options);
    let the_same_center = transform.get_lat_lng(WrapMode::Wrapped);
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

    transform.jump_to(&cam().with_center(Some(LatLng::default())).with_zoom(Some(10.0)));

    let mut true_center = transform.get_lat_lng(WrapMode::Wrapped);
    assert_double_eq(0.0, true_center.latitude());
    assert_double_eq(0.0, true_center.longitude());
    assert_double_eq(10.0, transform.get_zoom());

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

        true_center = transform.get_lat_lng(WrapMode::Wrapped);
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
    let start = cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(10.0)).with_pitch(Some(60.0));

    // Panning "up" the screen on a north-up pitched map moves the center north.
    transform.jump_to(&start);
    transform.move_by(&sc(0.0, 300.0), &AnimationOptions::default()).unwrap();
    let moderate_lat = transform.get_lat_lng(WrapMode::Wrapped).latitude();

    // A fling-sized offset toward the horizon must keep moving the center the same
    // way (north) -- before the fix it reversed south and stay bounded (clamped
    // short of the horizon).
    transform.jump_to(&start);
    transform.move_by(&sc(0.0, 100000.0), &AnimationOptions::default()).unwrap();
    let fling_lat = transform.get_lat_lng(WrapMode::Wrapped).latitude();

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

    transform.jump_to(&cam().with_center(Some(LatLng::default())).with_zoom(Some(1.0)));

    // San Francisco
    let coordinate_san_francisco = ll(37.7833, -122.4167);
    let pixel_sf = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_double_eq(151.79249437176432, pixel_sf.x);
    assert_double_eq(383.76720782527661, pixel_sf.y);

    transform.jump_to(&cam().with_center(Some(ll(0.0, -181.0))));

    let pixel_sf_longest = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_double_eq(-357.36306616412816, pixel_sf_longest.x);
    assert_double_eq(pixel_sf.y, pixel_sf_longest.y);
    let mut unwrapped_sf = coordinate_san_francisco.wrapped();
    unwrapped_sf.unwrap_for_shortest_path(&transform.get_lat_lng(WrapMode::Wrapped));

    let pixel_sf_shortest = transform.lat_lng_to_screen_coordinate(&unwrapped_sf);
    assert_double_eq(666.63694385219173, pixel_sf_shortest.x);
    assert_double_eq(pixel_sf.y, pixel_sf_shortest.y);

    transform.jump_to(&cam().with_center(Some(ll(0.0, 179.0))));
    let pixel_sf_shortest = transform.lat_lng_to_screen_coordinate(&coordinate_san_francisco);
    assert_double_eq(pixel_sf_longest.x, pixel_sf_shortest.x);
    assert_double_eq(pixel_sf_longest.y, pixel_sf_shortest.y);

    // Waikiri
    let coordinate_waikiri = ll(-16.9310, 179.9787);
    transform.jump_to(&cam().with_center(Some(coordinate_waikiri)).with_zoom(Some(10.0)));
    let pixel_waikiri = transform.lat_lng_to_screen_coordinate(&coordinate_waikiri);
    assert_double_eq(500.0, pixel_waikiri.x);
    assert_double_eq(500.0, pixel_waikiri.y);

    transform.jump_to(&cam().with_center(Some(ll(coordinate_waikiri.latitude(), 180.0213))));
    let pixel_waikiri_longest = transform.lat_lng_to_screen_coordinate(&coordinate_waikiri);
    assert_double_eq(524725.96438108233, pixel_waikiri_longest.x);
    assert_double_eq(pixel_waikiri.y, pixel_waikiri_longest.y);

    let mut unwrapped_waikiri = coordinate_waikiri.wrapped();
    unwrapped_waikiri.unwrap_for_shortest_path(&transform.get_lat_lng(WrapMode::Wrapped));
    let pixel_waikiri_shortest = transform.lat_lng_to_screen_coordinate(&unwrapped_waikiri);
    assert_double_eq(437.95925272648344, pixel_waikiri_shortest.x);
    assert_double_eq(pixel_waikiri.y, pixel_waikiri_shortest.y);

    let coordinate_from_pixel =
        transform.screen_coordinate_to_lat_lng(&pixel_waikiri_longest, WrapMode::Wrapped).unwrap();
    assert_near(coordinate_waikiri.latitude(), coordinate_from_pixel.latitude(), 1e-4);
    assert_near(coordinate_waikiri.longitude(), coordinate_from_pixel.longitude(), 1e-4);

    transform.jump_to(&cam().with_center(Some(ll(coordinate_waikiri.latitude(), 180.0213))));
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
        transform.jump_to(&cam().with_center(Some(ll(0.0, 179.9))).with_zoom(Some(5.0)));
        transform.jump_to(&cam().with_zoom(Some(6.0)).with_anchor(Some(sc(230.0, 128.0))));

        let longitude = transform.get_lat_lng(WrapMode::Unwrapped).longitude();
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
    let camera_options1 = cam().with_center(Some(lat_lng1)).with_zoom(Some(20.0));
    transform.jump_to(&camera_options1);
    assert_double_eq(lat_lng1.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng1.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(20.0, transform.get_zoom());

    let lat_lng2 = ll(-45.0, -135.0);
    let camera_options2 = cam().with_center(Some(lat_lng2)).with_zoom(Some(10.0));
    transform.jump_to(&camera_options2);
    assert_double_eq(lat_lng2.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(lat_lng2.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(10.0, transform.get_zoom());

    let mut ease_options = AnimationOptions::new(seconds(1));
    let ease_log = CallbackLog::install(&mut ease_options);
    let check_ease = |transform: &Transform| {
        for t in ease_log.take_frames() {
            assert!(t >= 0.0 && t <= 1.0, "t = {t}");
            assert!(lat_lng1.latitude() >= transform.get_lat_lng(WrapMode::Wrapped).latitude());
            assert!(lat_lng1.longitude() <= transform.get_lat_lng(WrapMode::Wrapped).longitude());
        }
        if ease_log.take_finish() {
            assert_double_eq(lat_lng1.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
            assert_double_eq(lat_lng1.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());
            assert_double_eq(20.0, transform.get_zoom());
        }
    };

    transform.ease_to(&camera_options1, &ease_options);
    check_ease(&transform);
    assert!(transform.in_transition());
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    check_ease(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    check_ease(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    check_ease(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    check_ease(&transform);
    assert!(!transform.in_transition());

    let mut fly_options = AnimationOptions::new(seconds(1));
    let fly_log = CallbackLog::install(&mut fly_options);
    let check_fly = |transform: &Transform| {
        for t in fly_log.take_frames() {
            assert!(t >= 0.0 && t <= 1.0, "t = {t}");
            assert!(lat_lng2.latitude() <= transform.get_lat_lng(WrapMode::Wrapped).latitude());
            assert!(lat_lng2.longitude() >= transform.get_lat_lng(WrapMode::Unwrapped).longitude());
        }
        if fly_log.take_finish() {
            // XXX Fix precision loss in flyTo:
            // https://github.com/mapbox/mapbox-gl-native/issues/4298
            assert_double_eq(lat_lng2.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude());
            assert_double_eq(lat_lng2.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude());
            assert_near(10.0, transform.get_zoom(), 1e-5);
        }
    };

    transform.fly_to(&camera_options2, &fly_options, false);
    check_fly(&transform);
    assert!(transform.in_transition());
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    check_fly(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    check_fly(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    check_fly(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    check_fly(&transform);
    assert!(!transform.in_transition());

    // Anchor and center points are mutually exclusive.
    let mut camera = CameraOptions::default();
    camera.center = Some(ll(0.0, 0.0));
    camera.anchor = Some(sc(0.0, 0.0)); // top-left
    camera.zoom = Some(transform.get_state().get_max_zoom());
    transform.ease_to(&camera, &AnimationOptions::new(seconds(1)));
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.0);
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.0);
}

// gtest: Transform.ProjectionMode
#[test]
fn projection_mode() {
    let mut transform = new_transform();

    transform.set_projection_mode(&ProjectionMode::default().with_axonometric(true).with_x_skew(1.0).with_y_skew(0.0));
    let options = transform.get_projection_mode();

    assert!(options.axonometric.unwrap());
    assert_eq!(options.x_skew.unwrap(), 1.0);
    assert_eq!(options.y_skew.unwrap(), 0.0);
}

// gtest: Transform.IsPanning
#[test]
fn is_panning() {
    let mut transform = new_transform();

    let mut ease_options = AnimationOptions::new(seconds(1));
    let log = CallbackLog::install(&mut ease_options);
    let check = |transform: &Transform| {
        for _ in log.take_frames() {
            assert!(transform.get_state().is_panning());
        }
        log.take_finish();
    };

    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.ease_to(&cam().with_center(Some(ll(0.0, 360.0))), &ease_options);
    check(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    check(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    check(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    check(&transform);
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    check(&transform);
}

// gtest: Transform.DefaultTransform
#[test]
fn default_transform() {
    #[derive(Default)]
    struct CountingObserver {
        camera_will_change_count: Cell<u32>,
        camera_did_change_count: Cell<u32>,
    }

    impl TransformObserver for CountingObserver {
        fn on_camera_will_change(&self, _change: CameraChange) {
            self.camera_will_change_count.set(self.camera_will_change_count.get() + 1);
        }

        fn on_camera_did_change(&self, _change: CameraChange) {
            self.camera_did_change_count.set(self.camera_did_change_count.get() + 1);
        }
    }

    let observer = Rc::new(CountingObserver::default());

    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    assert!(!transform.get_state().valid());

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

    match transform.resize(valid_size) {
        Ok(()) => {
            assert_eq!(observer.camera_will_change_count.get(), 1);
            assert_eq!(observer.camera_did_change_count.get(), 1);
        }
        Err(_) => panic!("Should not throw"),
    }

    assert!(transform.get_state().valid());

    // Double resize
    match transform.resize(valid_size) {
        Ok(()) => {
            assert_eq!(observer.camera_will_change_count.get(), 1);
            assert_eq!(observer.camera_did_change_count.get(), 1);
        }
        Err(_) => panic!("Should not throw"),
    }

    let center = sc(max as f64 / 2.0, max as f64 / 2.0);
    let lat_lng = transform.get_state().screen_coordinate_to_lat_lng(&center, WrapMode::Unwrapped).unwrap();
    assert_near(lat_lng.latitude(), null_island.latitude(), 1e-8);
    assert_near(lat_lng.longitude(), null_island.longitude(), 1e-8);

    let point = transform.get_state().lat_lng_to_screen_coordinate(&null_island);
    assert_double_eq(point.x, center.x);
    assert_double_eq(point.y, center.y);

    // Constrain to screen while resizing
    transform.resize(Size::new(1000, 500)).unwrap();
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0))).unwrap();
    transform.set_constrain_mode(ConstrainMode::Screen);

    // Request impossible zoom
    let ease_options = AnimationOptions::new(seconds(1));
    transform.ease_to(&cam().with_center(Some(ll(56.0, 11.0))).with_zoom(Some(1.0)), &ease_options);
    assert!(transform.in_transition());
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));

    // Rotate the screen during a transition (resize it)
    transform.resize(Size::new(500, 1000)).unwrap();

    // The resize while constraining to screen should have stopped the transition and updated the state
    assert!(!transform.in_transition());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 8.22103, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 46.6905, 1e-4);
    assert_near(transform.get_state().get_scale(), 38.1529, 1e-4);
}

// gtest: Transform.LatLngBounds
#[test]
fn lat_lng_bounds() {
    let null_island = LatLng::default();
    let san_francisco = ll(37.7749, -122.4194);

    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();

    let max_zoom = transform.get_state().get_max_zoom();
    transform.jump_to(&cam().with_center(Some(LatLng::default())).with_zoom(Some(max_zoom)));

    // Default bounds.
    assert_eq!(transform.get_state().get_lat_lng_bounds(), LatLngBounds::default());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), null_island);

    // Invalid bounds.
    match transform.set_lat_lng_bounds(LatLngBounds::empty()) {
        Ok(()) => panic!("Should throw"),
        Err(_) => assert_eq!(transform.get_state().get_lat_lng_bounds(), LatLngBounds::default()),
    }

    transform.jump_to(&cam().with_center(Some(san_francisco)));
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), san_francisco.latitude(), 1e-8);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), san_francisco.longitude(), 1e-8);

    // Single location.
    transform.set_lat_lng_bounds(LatLngBounds::singleton(san_francisco)).unwrap();
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), san_francisco);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃▓▓▓│▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, -180.0), ll(0.0, 180.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(san_francisco)));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), san_francisco.longitude());

    // Try crossing the antimeridian from the left.
    transform.jump_to(&cam().with_center(Some(ll(0.0, -200.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -180.0);

    // Try crossing the antimeridian from the right.
    transform.jump_to(&cam().with_center(Some(ll(0.0, 200.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 180.0);
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -180.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │▓▓▓┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, 0.0), ll(90.0, 180.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(san_francisco)));
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), san_francisco.latitude(), 1e-8);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃•  │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │▓▓▓┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, 0.0), ll(0.0, 180.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(san_francisco)));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃   │  ▓┃▓  │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │   ┃   │   ┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    let inside = ll(45.0, 150.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(0.0, 120.0), ll(90.0, 240.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(inside)));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), inside.longitude());

    transform.jump_to(&cam().with_center(Some(ll(0.0, 140.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 140.0);

    transform.jump_to(&cam().with_center(Some(ll(0.0, 160.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 160.0);

    // Constrain latitude only.
    transform.jump_to(&cam().with_center(Some(ll(-45.0, inside.longitude()))));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), inside.longitude());

    // Crossing the antimeridian, within bounds.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), 181.0))));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -179.0);

    // Crossing the antimeridian, outside bounds.
    transform.jump_to(&cam().with_center(Some(inside)));
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), 250.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -120.0);

    // Constrain to the left edge.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), 119.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 120.0);

    // Simulate swipe to the left.
    let mut ease_options = AnimationOptions::new(seconds(1));
    let log = CallbackLog::install(&mut ease_options);
    let check_swipe = |transform: &Transform, longitude: f64| {
        for _ in log.take_frames() {
            assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), longitude, 1e-4);
        }
        if log.take_finish() {
            assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), longitude, 1e-4);
        }
    };
    transform.move_by(&sc(-500.0, -500.0), &ease_options).unwrap();
    check_swipe(&transform, 120.0);

    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(0));
    check_swipe(&transform, 120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    check_swipe(&transform, 120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    check_swipe(&transform, 120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    check_swipe(&transform, 120.0);
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    check_swipe(&transform, 120.0);

    // Constrain to the right edge.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), 241.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -120.0);

    // Simulate swipe to the right.
    transform.move_by(&sc(500.0, 500.0), &ease_options).unwrap();
    check_swipe(&transform, -120.0);

    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(0));
    check_swipe(&transform, -120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    check_swipe(&transform, -120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(500));
    check_swipe(&transform, -120.0);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(750));
    check_swipe(&transform, -120.0);
    transform.update_transitions(transform.get_latest_transition_start() + transform.get_latest_transition_duration());
    check_swipe(&transform, -120.0);

    //    -1   |   0   |  +1
    // ┌───┬───┰───┬───┰───┬───┐
    // │   │   ┃   │   ┃   │   │
    // ├───┼───╂───┼───╂───┼───┤
    // │   │  ▓┃▓  │   ┃   │   │
    // └───┴───┸───┴───┸───┴───┘
    let inside = ll(-45.0, -150.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-90.0, -240.0), ll(0.0, -120.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(inside)));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), inside.longitude());

    transform.jump_to(&cam().with_center(Some(ll(0.0, -140.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -140.0);

    transform.jump_to(&cam().with_center(Some(ll(0.0, -160.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -160.0);

    // Constrain latitude only.
    transform.jump_to(&cam().with_center(Some(ll(45.0, inside.longitude()))));
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 0.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), inside.longitude());

    // Crossing the antimeridian, within bounds.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), -181.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).latitude(), inside.latitude());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 179.0);

    // Crossing the antimeridian, outside bounds.
    transform.jump_to(&cam().with_center(Some(inside)));
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), -250.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 120.0);

    // Constrain to the left edge.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), -119.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -120.0);

    transform.move_by(&sc(-500.0, 0.0), &AnimationOptions::default()).unwrap();
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -120.0);

    // Constrain to the right edge.
    transform.jump_to(&cam().with_center(Some(ll(inside.latitude(), -241.0))));
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 120.0);

    transform.move_by(&sc(500.0, 0.0), &AnimationOptions::default()).unwrap();
    assert_double_eq(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 120.0);
}

// Port-only: the floor screen bounds set is the zoom a request below it
// settles on, wherever the center is; the width decides on a square screen
// and the height on a tall one.
#[test]
fn min_zoom_for_screen_bounds() {
    let mut transform = new_transform();
    transform.resize(Size::new(500, 500)).unwrap();
    assert_eq!(None, transform.get_state().get_min_zoom_for_screen_bounds());
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(40.0, -10.0), ll(70.0, 40.0))).unwrap();
    assert_eq!(None, transform.get_state().get_min_zoom_for_screen_bounds());
    transform.set_constrain_mode(ConstrainMode::Screen);
    for (size, floor) in [(Size::new(500, 500), 2.81378), (Size::new(500, 1000), 3.65750)] {
        transform.resize(size).unwrap();
        let min_zoom = transform.get_state().get_min_zoom_for_screen_bounds().unwrap();
        assert_near(min_zoom, floor, 1e-4);
        for center in [ll(56.0, 11.0), ll(45.0, -5.0)] {
            transform.ease_to(&cam().with_center(Some(center)).with_zoom(Some(1.0)), &AnimationOptions::default());
            assert_near(transform.get_zoom(), min_zoom, 1e-9);
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
    transform.ease_to(&cam().with_center(Some(ll(56.0, 11.0))).with_zoom(Some(1.0)), &AnimationOptions::default());
    assert_near(transform.get_zoom(), 2.81378, 1e-4);

    // Request impossible center left
    transform.ease_to(&cam().with_center(Some(ll(56.0, -65.0))).with_zoom(Some(4.0)), &AnimationOptions::default());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center top
    transform.ease_to(&cam().with_center(Some(ll(80.0, 11.0))).with_zoom(Some(4.0)), &AnimationOptions::default());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 11.0, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 65.88603, 1e-4);

    // Request impossible center right
    transform.ease_to(&cam().with_center(Some(ll(56.0, 50.0))).with_zoom(Some(4.0)), &AnimationOptions::default());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 29.01367, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center bottom
    transform.ease_to(&cam().with_center(Some(ll(30.0, 11.0))).with_zoom(Some(4.0)), &AnimationOptions::default());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 11.0, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 47.89217, 1e-4);

    // Request impossible center with anchor
    transform.ease_to(
        &cam().with_anchor(Some(sc(250.0, 250.0))).with_center(Some(ll(56.0, -65.0))).with_zoom(Some(4.0)),
        &AnimationOptions::default(),
    );
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Request impossible center (anchor)
    transform.ease_to(&cam().with_anchor(Some(sc(250.0, 250.0))).with_zoom(Some(4.0)), &AnimationOptions::default());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Fly to impossible center
    transform.fly_to(
        &cam().with_center(Some(ll(56.0, -65.0))).with_zoom(Some(4.0)),
        &AnimationOptions::default(),
        false,
    );
    assert_near(transform.get_zoom(), 4.0, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);

    // Fly to impossible center and zoom
    transform.fly_to(
        &cam().with_center(Some(ll(56.0, -65.0))).with_zoom(Some(2.0)),
        &AnimationOptions::default(),
        false,
    );
    assert_near(transform.get_zoom(), 4.0, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 0.98632, 1e-4);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 56.0, 1e-4);
}

// gtest: Transform.InvalidPitch
#[test]
fn invalid_pitch() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.get_zoom());
    assert_double_eq(0.0, transform.get_pitch());

    transform.jump_to(&cam().with_zoom(Some(1.0)).with_pitch(Some(45.0)));
    assert_double_eq(1.0, transform.get_zoom());
    assert_double_eq(deg2rad(45.0), transform.get_pitch());

    let invalid = f64::NAN;

    transform.jump_to(&cam().with_pitch(Some(invalid)));
    assert_double_eq(deg2rad(45.0), transform.get_pitch());

    transform.jump_to(&cam().with_pitch(Some(60.0)));
    assert_double_eq(deg2rad(60.0), transform.get_pitch());
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
            transform.jump_to(
                &cam().with_center(Some(ll(40.7128, -74.006))).with_zoom(Some(9.5)).with_pitch(Some(min_pitch)),
            );

            transform.jump_to(&cam().with_padding(Some(insets(size as f64 * 28.0, 392.0, 0.0, 0.0))));
            assert_double_eq(deg2rad(min_pitch), transform.get_pitch());

            transform.jump_to(&cam().with_padding(Some(insets(size as f64 * 24.0, 392.0, 0.0, 0.0))));
            assert_double_eq(deg2rad(min_pitch), transform.get_pitch());
            assert_double_eq(size as f64 * 24.0, transform.get_state().get_edge_insets().top());

            transform.resize(Size::new(800, 600)).unwrap();
            assert_double_eq(deg2rad(min_pitch), transform.get_pitch());
            transform.jump_to(&cam().with_padding(Some(EdgeInsets::default())).with_pitch(Some(45.0)));
            assert_double_eq(deg2rad(45.0), transform.get_pitch());
        }
    }
}

// gtest: Transform.MinMaxPitch
#[test]
fn min_max_pitch() {
    let mut transform = new_transform();
    transform.resize(Size::new(1, 1)).unwrap();

    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_double_eq(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude());
    assert_double_eq(0.0, transform.get_zoom());
    assert_double_eq(0.0, transform.get_pitch());

    transform.jump_to(&cam().with_zoom(Some(1.0)).with_pitch(Some(60.0)));
    assert_double_eq(1.0, transform.get_zoom());
    assert_double_eq(transform.get_state().get_max_pitch(), transform.get_pitch());
    assert_double_eq(deg2rad(60.0), transform.get_pitch());

    transform.set_max_pitch(70.0);
    transform.jump_to(&cam().with_pitch(Some(70.0)));
    assert_double_eq(transform.get_state().get_max_pitch(), transform.get_pitch());

    transform.set_max_pitch(45.0);
    transform.jump_to(&cam().with_pitch(Some(60.0)));
    assert_double_eq(transform.get_state().get_max_pitch(), transform.get_pitch());
    assert_double_eq(deg2rad(45.0), transform.get_pitch());

    transform.jump_to(&cam().with_pitch(Some(0.0)));
    assert_double_eq(transform.get_state().get_min_pitch(), transform.get_pitch());
    assert_double_eq(0.0, transform.get_pitch());

    transform.set_min_pitch(-10.0);
    transform.jump_to(&cam().with_pitch(Some(-10.0)));
    assert_double_eq(transform.get_state().get_min_pitch(), transform.get_pitch());
    assert_double_eq(0.0, transform.get_pitch());

    transform.set_min_pitch(15.0);
    transform.jump_to(&cam().with_pitch(Some(0.0)));
    assert_double_eq(transform.get_state().get_min_pitch(), transform.get_pitch());
    assert_double_eq(deg2rad(15.0), transform.get_pitch());

    transform.set_min_pitch(45.0);
    assert_double_eq(deg2rad(45.0), transform.get_state().get_min_pitch());
    transform.set_max_pitch(45.0);
    assert_double_eq(deg2rad(45.0), transform.get_state().get_max_pitch());

    transform.set_max_pitch(10.0);
    assert_double_eq(deg2rad(45.0), transform.get_state().get_max_pitch());

    transform.set_min_pitch(60.0);
    assert_double_eq(deg2rad(45.0), transform.get_state().get_min_pitch());
}

// gtest: Transform.FreeCameraOptionsInvalidSize
#[test]
fn free_camera_options_invalid_size() {
    let mut transform = new_transform();
    let mut options = FreeCameraOptions::default();

    options.orientation = Some([1.0, 1.0, 1.0, 1.0]);
    options.position = Some([0.1, 0.2, 0.3]);
    transform.set_free_camera_options(&options);

    let updated_orientation = transform.get_free_camera_options().orientation.unwrap();
    let updated_position = transform.get_free_camera_options().position.unwrap();

    assert_double_eq(0.0, updated_orientation[0]);
    assert_double_eq(0.0, updated_orientation[1]);
    assert_double_eq(0.0, updated_orientation[2]);
    assert_double_eq(1.0, updated_orientation[3]);

    assert_vec3_near_1e5(&updated_position, &[0.0, 0.0, 0.0]);
}

// gtest: Transform.FreeCameraOptionsNanInput
#[test]
fn free_camera_options_nan_input() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    let mut options = FreeCameraOptions::default();

    options.position = Some([0.5, 0.5, 0.25]);
    transform.set_free_camera_options(&options);

    options.position = Some([0.0, 0.0, f64::NAN]);
    transform.set_free_camera_options(&options);
    assert_eq!([0.5, 0.5, 0.25], transform.get_free_camera_options().position.unwrap());

    // Only the invalid parameter should be discarded
    options.position = Some([0.3, 0.1, 0.2]);
    options.orientation = Some([f64::NAN, 0.0, f64::NAN, 0.0]);
    transform.set_free_camera_options(&options);
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.3, 0.1, 0.2]);
    assert_eq!(Quaternion::IDENTITY.m(), transform.get_free_camera_options().orientation.unwrap());
}

// gtest: Transform.FreeCameraOptionsInvalidZ
#[test]
fn free_camera_options_invalid_z() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    let mut options = FreeCameraOptions::default();

    // Invalid z-value (<= 0.0 || > 1) should be clamped to respect both min&max zoom values
    options.position = Some([0.1, 0.1, 0.0]);
    transform.set_free_camera_options(&options);
    assert_double_eq(transform.get_state().get_max_zoom(), transform.get_state().get_zoom());
    assert!(transform.get_free_camera_options().position.unwrap()[2] > 0.0);

    options.position = Some([0.5, 0.2, 123.456]);
    transform.set_free_camera_options(&options);
    assert_double_eq(transform.get_state().get_min_zoom(), transform.get_state().get_zoom());
    assert!(transform.get_free_camera_options().position.unwrap()[2] <= 1.0);
}

// gtest: Transform.CameraFollowsJumpsBelowAFractionalMinimumZoom
#[test]
fn camera_follows_jumps_below_a_fractional_minimum_zoom() {
    let mercator_y = 0.5 - (PI / 4.0 + deg2rad(10.0) / 2.0).tan().ln() / (2.0 * PI);
    for min_zoom in [1.525373023184723, 1.55, 3.1, 6.2] {
        let _trace = ScopedTrace(std_to_string(min_zoom));
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_min_zoom(min_zoom);
        transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(min_zoom + 2.0)));
        transform.jump_to(&cam().with_center(Some(ll(10.0, 20.0))).with_zoom(Some(0.0)));
        assert!(transform.get_state().valid());
        let position = transform.get_free_camera_options().position.unwrap();
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
    let mut options = FreeCameraOptions::default();

    options.position = Some([0.1, 0.1, 0.1]);
    options.orientation = Some(Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], deg2rad(-90.0)).m());
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(90.0), transform.get_state().get_pitch());
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.1, 0.1, 0.1]);

    options.position = Some([0.1, 0.1, 0.1]);
    options.orientation = Some(Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], deg2rad(-135.0)).m());
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(135.0), transform.get_state().get_pitch());
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.1, 0.1, 0.1]);
}

// gtest: Transform.FreeCameraOptionsInvalidOrientation
#[test]
fn free_camera_options_invalid_orientation() {
    // Invalid orientations that cannot be clamped into a valid range
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();

    let mut options = FreeCameraOptions::default();
    options.orientation = Some([0.0, 0.0, 0.0, 0.0]);
    transform.set_free_camera_options(&options);
    assert_eq!(Some(Quaternion::IDENTITY.m()), transform.get_free_camera_options().orientation);

    // Gimbal lock. Both forward and up vectors are on xy-plane
    transform.set_max_pitch(180.0);
    options.orientation = Some(Quaternion::from_axis_angle(&[0.0, 1.0, 0.0], PI / 2.0).m());
    transform.set_free_camera_options(&options);
    for i in 0..4 {
        assert_near(
            Quaternion::from_euler_angles(PI / 2.0, PI / 2.0, PI / 2.0).m()[i],
            transform.get_free_camera_options().orientation.unwrap()[i],
            1.0e-12,
        );
    }

    // Camera is upside down
    options.orientation = Some(Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], PI / 2.0 + PI / 4.0).m());
    transform.set_free_camera_options(&options);
    for i in 0..4 {
        assert_near(
            options.orientation.unwrap()[i],
            transform.get_free_camera_options().orientation.unwrap()[i],
            1.0e-12,
        );
    }
}

// gtest: Transform.FreeCameraOptionsSetOrientation
#[test]
fn free_camera_options_set_orientation() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    let mut options = FreeCameraOptions::default();

    options.orientation = Some(Quaternion::IDENTITY.m());
    transform.set_free_camera_options(&options);
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_double_eq(0.0, transform.get_state().get_pitch());
    assert_double_eq(0.0, transform.get_state().get_x());
    assert_double_eq(0.0, transform.get_state().get_y());

    options.orientation = Some(Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], deg2rad(-60.0)).m());
    transform.set_free_camera_options(&options);
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_double_eq(deg2rad(60.0), transform.get_state().get_pitch());
    assert_double_eq(0.0, transform.get_state().get_x());
    assert_double_eq(206.0, transform.get_state().get_y());

    options.orientation = Some(Quaternion::from_axis_angle(&[0.0, 0.0, 1.0], deg2rad(56.0)).m());
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(-56.0), transform.get_state().get_bearing());
    assert_double_eq(0.0, transform.get_state().get_pitch());
    assert_double_eq(0.0, transform.get_state().get_roll());
    assert_double_eq(0.0, transform.get_state().get_x());
    assert_near(152.192378, transform.get_state().get_y(), 1e-6);

    options.orientation = Some(
        Quaternion::from_euler_angles(0.0, 0.0, deg2rad(-179.0))
            .multiply(&Quaternion::from_euler_angles(deg2rad(-30.0), 0.0, 0.0))
            .m(),
    );
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(179.0), transform.get_state().get_bearing());
    assert_double_eq(deg2rad(30.0), transform.get_state().get_pitch());
    assert_near(1.308930, transform.get_state().get_x(), 1e-6);
    assert_near(56.813889, transform.get_state().get_y(), 1e-6);

    options.orientation = Some(
        Quaternion::from_axis_angle(&[0.0, 0.0, 1.0], deg2rad(-33.0))
            .multiply(&Quaternion::from_axis_angle(&[1.0, 0.0, 0.0], deg2rad(-22.0)))
            .multiply(&Quaternion::from_axis_angle(&[0.0, 0.0, 1.0], deg2rad(11.0)))
            .m(),
    );
    transform.set_free_camera_options(&options);
    assert_double_eq(deg2rad(33.0), transform.get_state().get_bearing());
    assert_double_eq(deg2rad(22.0), transform.get_state().get_pitch());
    assert_float_eq(deg2rad(11.0), transform.get_state().get_roll());
}

// gtest: Transform.FreeCameraOptionsClampToBounds
#[test]
// The C++ test's expected values, rounded to six digits.
#[allow(clippy::approx_constant)]
fn free_camera_options_clamp_to_bounds() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();
    transform.set_constrain_mode(ConstrainMode::WidthAndHeight);
    transform.jump_to(&cam().with_zoom(Some(8.56)));
    let mut options = FreeCameraOptions::default();

    // Place camera to an arbitrary position looking away from the map
    options.position = Some([-100.0, -10000.0, 1000.0]);
    options.orientation = Some(Quaternion::from_euler_angles(deg2rad(-45.0), 0.0, 0.0).m());
    transform.set_free_camera_options(&options);

    // Map center should be clamped to width/2 pixels away from map borders
    assert_double_eq(206.0, transform.get_state().get_x());
    assert_double_eq(206.0, transform.get_state().get_y());
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_double_eq(deg2rad(45.0), transform.get_state().get_pitch());

    let (right, up, forward) = rotated_frame(&transform.get_free_camera_options().orientation.unwrap());
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.0976562, 0.304816, 0.20716]);
    assert_vec3_near_1e5(&right, &[1.0, 0.0, 0.0]);
    assert_vec3_near_1e5(&up, &[0.0, -0.707107, 0.707107]);
    assert_vec3_near_1e5(&forward, &[0.0, -0.707107, -0.707107]);
}

// gtest: Transform.FreeCameraOptionsInvalidState
#[test]
fn free_camera_options_invalid_state() {
    let transform = new_transform();

    // Invalid size
    assert_double_eq(0.0, transform.get_state().get_x());
    assert_double_eq(0.0, transform.get_state().get_y());
    assert_double_eq(0.0, transform.get_state().get_bearing());
    assert_double_eq(0.0, transform.get_state().get_pitch());

    let options = transform.get_free_camera_options();
    assert_vec3_near_1e5(&options.position.unwrap(), &[0.0, 0.0, 0.0]);
}

// gtest: Transform.FreeCameraOptionsStateSynchronization
#[test]
fn free_camera_options_state_synchronization() {
    let mut transform = new_transform();
    transform.resize(Size::new(100, 100)).unwrap();

    transform.jump_to(&cam().with_pitch(Some(0.0)).with_bearing(Some(0.0)));
    let (right, up, forward) = rotated_frame(&transform.get_free_camera_options().orientation.unwrap());
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.5, 0.5, 0.29296875]);
    assert_vec3_near_1e5(&right, &[1.0, 0.0, 0.0]);
    assert_vec3_near_1e5(&up, &[0.0, -1.0, 0.0]);
    assert_vec3_near_1e5(&forward, &[0.0, 0.0, -1.0]);

    transform.jump_to(&cam().with_center(Some(ll(60.1699, 24.9384))));
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.569273, 0.289453, 0.292969]);

    transform.jump_to(&cam().with_pitch(Some(20.0)).with_bearing(Some(77.0)).with_center(Some(ll(-20.0, 20.0))));
    assert_vec3_near_1e5(&transform.get_free_camera_options().position.unwrap(), &[0.457922, 0.57926, 0.275301]);
}

// TEST(Camera, *) (util::Camera)

// gtest: Camera.SetOrientation
#[test]
fn set_orientation() {
    let mut camera = UtilCamera::new();
    let bearing = 0.22;
    let pitch = 0.34;
    let roll = 0.0;
    camera.set_orientation(roll, pitch, bearing);
    let (roll_, pitch_, bearing_) = camera.get_orientation_angles();
    assert_near(bearing, bearing_, 1.0e-9);
    assert_near(pitch, pitch_, 1.0e-9);
    assert_near(roll, roll_, 1.0e-9);
}

// gtest: Camera.SetOrientationWithRoll
#[test]
fn set_orientation_with_roll() {
    let mut camera = UtilCamera::new();
    let bearing = 0.22;
    let pitch = 0.34;
    let roll = 0.45;
    camera.set_orientation(roll, pitch, bearing);
    let (roll_, pitch_, bearing_) = camera.get_orientation_angles();
    assert_near(bearing, bearing_, 1.0e-9);
    assert_near(pitch, pitch_, 1.0e-9);
    assert_near(roll, roll_, 1.0e-9);
}

// gtest: Camera.SetOrientationWithRollNoPitch
#[test]
fn set_orientation_with_roll_no_pitch() {
    let mut camera = UtilCamera::new();
    let bearing = 0.22;
    let pitch = 0.0;
    let roll = 0.45;
    camera.set_orientation(roll, pitch, bearing);
    let (roll_, pitch_, bearing_) = camera.get_orientation_angles();
    assert_near(bearing - roll, bearing_, 1.0e-9);
    assert_near(pitch, pitch_, 1.0e-9);
    assert_near(0.0, roll_, 1.0e-9);
}

/// The tile matrix as `TransformState::matrix_for` computed it before the
/// projection seam.
fn reference_tile_matrix(scale: f64, tile_id: &UnwrappedTileID) -> Mat4 {
    let tile_scale: u64 = 1u64 << tile_id.canonical.z;
    let s = Projection::world_size(scale) / tile_scale as f64;

    let mut m: Mat4 = [0.0; 16];
    matrix::identity(&mut m);
    let a = m;
    matrix::translate(
        &mut m,
        &a,
        (tile_id.canonical.x as i64 + tile_id.wrap as i64 * tile_scale as i64) as f64 * s,
        tile_id.canonical.y as f64 * s,
        0.0,
    );
    let a = m;
    matrix::scale(&mut m, &a, s / EXTENT as f64, s / EXTENT as f64, 1.0);
    m
}

// gtest: MercatorProjection.MatchesStaticProjection
#[test]
fn matches_static_projection() {
    let projection = MercatorProjection;
    for zoom in [0.0, 1.0, 3.5, 7.25, 12.0, 18.0] {
        let scale = 2.0f64.powf(zoom);
        for lat in [-85.0, -45.0, 0.0, 30.0, 80.0] {
            for lon in [-180.0, -90.0, 0.0, 45.0, 179.0] {
                let lat_lng = ll(lat, lon);
                let expected = Projection::project(&lat_lng, scale);
                let actual = projection.project(&lat_lng, scale);
                assert_eq!(expected.x, actual.x);
                assert_eq!(expected.y, actual.y);

                let unprojected = projection.unproject(&actual, scale, WrapMode::Wrapped).unwrap();
                let expected_unprojected = Projection::unproject(&expected, scale, WrapMode::Wrapped).unwrap();
                assert_eq!(expected_unprojected.latitude(), unprojected.latitude());
                assert_eq!(expected_unprojected.longitude(), unprojected.longitude());
            }
        }
    }
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
                        transform.jump_to(
                            &cam()
                                .with_center(Some(ll(37.7749, -122.4194)))
                                .with_zoom(Some(zoom))
                                .with_pitch(Some(pitch))
                                .with_bearing(Some(bearing))
                                .with_roll(Some(roll)),
                        );
                        let state = transform.get_state();
                        let scale = state.get_scale();

                        for aligned in [false, true] {
                            let mut proj_matrix: Mat4 = [0.0; 16];
                            state.get_proj_matrix(&mut proj_matrix, 1, aligned);

                            for tile_id in &tile_ids {
                                let tile_matrix = reference_tile_matrix(scale, tile_id);
                                let mut actual_tile_matrix: Mat4 = [0.0; 16];
                                state.matrix_for(&mut actual_tile_matrix, tile_id);
                                assert_eq!(tile_matrix, actual_tile_matrix);

                                let mut expected: Mat4 = [0.0; 16];
                                matrix::multiply(&mut expected, &proj_matrix, &tile_matrix);
                                assert_eq!(expected, state.get_projection_data_with(tile_id, &proj_matrix).main_matrix);
                            }
                        }

                        for tile_id in &tile_ids {
                            let mut expected: Mat4 = [0.0; 16];
                            matrix::multiply(
                                &mut expected,
                                &state.get_projection_matrix(),
                                &reference_tile_matrix(scale, tile_id),
                            );
                            assert_eq!(expected, state.get_projection_data(tile_id).main_matrix);
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
    transform.jump_to(
        &cam().with_center(Some(ll(10.0, 20.0))).with_zoom(Some(5.0)).with_pitch(Some(20.0)).with_bearing(Some(30.0)),
    );
    let state = transform.get_state();

    for tile_id in [
        UnwrappedTileID::new(0, 0, 0),
        UnwrappedTileID::new(3, 5, 2),
        UnwrappedTileID::with_wrap(-1, CanonicalTileID::new(2, 1, 1)),
    ] {
        let data = state.get_projection_data(&tile_id);
        let tile_scale = (1u32 << tile_id.canonical.z) as f64;
        assert_eq!(data.main_matrix, data.fallback_matrix);
        assert_eq!(tile_id.canonical.x as f64 / tile_scale, data.tile_mercator_coords[0]);
        assert_eq!(tile_id.canonical.y as f64 / tile_scale, data.tile_mercator_coords[1]);
        assert_eq!(1.0 / tile_scale / EXTENT as f64, data.tile_mercator_coords[2]);
        assert_eq!(1.0 / tile_scale / EXTENT as f64, data.tile_mercator_coords[3]);
        assert_eq!([0.0, 0.0, 0.0, 0.0], data.clipping_plane);
        assert_eq!(0.0, data.projection_transition);
    }
}

// gtest: Transform.GlobeProjectionDefinition
#[test]
fn globe_projection_definition() {
    let mut transform = new_transform();
    assert!(!transform.get_state().is_globe_rendering());
    assert_double_eq(0.0, transform.get_state().get_projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 0.5));
    assert!(transform.get_state().is_globe_rendering());
    assert_double_eq(0.5, transform.get_state().get_projection_transition());
    // The transition is how far the globe has gone, whichever way the
    // definition reads.
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 0.25));
    assert_double_eq(0.75, transform.get_state().get_projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::between_names("mercator", "vertical-perspective", 0.25));
    assert_double_eq(0.25, transform.get_state().get_projection_transition());
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert!(!transform.get_state().is_globe_rendering());
    assert_double_eq(0.0, transform.get_state().get_projection_transition());
}

// A transition past either end, from a literal or an overshooting curve,
// holds at that end's projection.
// gtest: Transform.GlobeProjectionTransitionPastEitherEnd
#[test]
fn globe_projection_transition_past_either_end() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::between_names("mercator", "vertical-perspective", 1.5));
    assert_double_eq(1.0, transform.get_state().get_projection_transition());
    transform.jump_to(&cam().with_zoom(Some(1.0)));
    for step in 1..=3 {
        transform.jump_to(&cam().with_center(Some(ll(0.0, 40.0 + 120.0 * f64::from(step)))));
    }
    assert_near(400.0, transform.get_state().get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
    // Past Mercator the globe leaves as it does at 0, so the center wraps back
    // into Mercator's world.
    transform.set_projection_definition(&ProjectionDefinition::between_names("vertical-perspective", "mercator", 1.5));
    assert!(!transform.get_state().is_globe_rendering());
    assert_double_eq(0.0, transform.get_state().get_projection_transition());
    assert_near(40.0, transform.get_state().get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
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
                transform.jump_to(&cam().with_center(Some(center)).with_zoom(Some(5.0)));
                let camera = cam().with_center(Some(center)).with_zoom(Some(zoom));
                let animation = AnimationOptions::new(milliseconds(1000));
                if fly {
                    transform.fly_to(&camera, &animation, false);
                } else {
                    transform.ease_to(&camera, &animation);
                }
                // As the map does each frame, the projection follows the zoom.
                for frame in 1..=20 {
                    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(50 * frame));
                    let definition = projection_for_zoom(transform.get_zoom());
                    transform.set_projection_definition(&definition);
                }
                assert!(transform.get_state().is_globe_rendering());
                assert_near(zoom, transform.get_zoom(), 1e-9);
            }
        }
    }
}

// gtest: VerticalPerspectiveProjection.TileCoordinatesToSphere
#[test]
fn tile_coordinates_to_sphere() {
    let near =
        |a: &Vec3, b: &Vec3| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9 && (a[2] - b[2]).abs() < 1e-9;
    let world = UnwrappedTileID::new(0, 0, 0);
    let half = f64::from(EXTENT) / 2.0;
    // (0°, 0°) is the +Z axis; the equator runs through ±X; latitude is Y.
    assert!(near(
        &[0.0, 0.0, 1.0],
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, half), &world)
    ));
    assert!(near(
        &[1.0, 0.0, 0.0],
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half * 1.5, half), &world)
    ));
    assert!(near(
        &[-1.0, 0.0, 0.0],
        &VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half * 0.5, half), &world)
    ));
    let north = VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, 0.0), &world);
    assert!(north[1] > 0.99);
    let south = VerticalPerspectiveProjection::tile_coordinates_to_sphere(&Point::new(half, f64::from(EXTENT)), &world);
    assert!(south[1] < -0.99);
}

/// `setUpGlobe(transform, center, zoom, bearing = 0, pitch = 0)`.
fn set_up_globe(transform: &mut Transform, center: &LatLng, zoom: f64, bearing: f64, pitch: f64) {
    transform.resize(Size::new(800, 600)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(
        &cam().with_center(Some(*center)).with_zoom(Some(zoom)).with_bearing(Some(bearing)).with_pitch(Some(pitch)),
    );
}

/// The GL JS camera tests run on a 512 by 512 globe.
fn set_up_globe_camera(transform: &mut Transform, center: &LatLng, zoom: f64) {
    transform.resize(Size::new(512, 512)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&cam().with_center(Some(*center)).with_zoom(Some(zoom)));
}

/// One second with a linear easing, so a frame at half time is the frame at
/// k = 0.5.
fn linear_second() -> AnimationOptions {
    AnimationOptions {
        duration: Some(milliseconds(1000)),
        easing: Some(UnitBezier::new(0.0, 0.0, 1.0, 1.0)),
        ..AnimationOptions::default()
    }
}

fn run_to(transform: &mut Transform, k: f64) {
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds((k * 1000.0) as i64));
}

// gtest: VerticalPerspectiveProjection.CenterProjectsToScreenCenter
#[test]
fn center_projects_to_screen_center() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(37.0, -122.0), 3.0, 20.0, 30.0);
    let state = transform.get_state();
    assert!(state.is_globe_rendering());

    let radius = VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(state.get_scale()),
        state.get_lat_lng(WrapMode::Unwrapped).latitude(),
    );
    let m = VerticalPerspectiveProjection::globe_view_projection_matrix(state, radius);
    let center = VerticalPerspectiveProjection::surface_vector(&state.get_lat_lng(WrapMode::Unwrapped));
    let surface: Vec4 = [center[0], center[1], center[2], 1.0];
    let mut clip: Vec4 = [0.0; 4];
    matrix::transform_mat4(&mut clip, &surface, &m);
    assert_near(0.0, clip[0] / clip[3], 1e-6);
    assert_near(0.0, clip[1] / clip[3], 1e-6);

    let plane = VerticalPerspectiveProjection::clipping_plane(state, radius);
    assert!(plane[0] * surface[0] + plane[1] * surface[1] + plane[2] * surface[2] + plane[3] > 0.0);
    let antipode: Vec4 = [-surface[0], -surface[1], -surface[2], 1.0];
    assert!(plane[0] * antipode[0] + plane[1] * antipode[1] + plane[2] * antipode[2] + plane[3] < 0.0);
}

// gtest: GlobeTransform.CachedViewStateFollowsTheCamera
#[test]
fn cached_view_state_follows_the_camera() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(20.0, -40.0), 2.5, 35.0, 25.0);
    let check = |state: &TransformState| {
        let radius = VerticalPerspectiveProjection::globe_radius_pixels(
            Projection::world_size(state.get_scale()),
            state.get_lat_lng(WrapMode::Unwrapped).latitude(),
        );
        assert_double_eq(radius, state.get_globe_radius_pixels());
        let expected = VerticalPerspectiveProjection::globe_view_projection_matrix(state, radius);
        let plane = VerticalPerspectiveProjection::clipping_plane(state, radius);
        let camera = VerticalPerspectiveProjection::camera_position(state, radius);
        for i in 0..16 {
            let _trace = ScopedTrace(format!("element {i}"));
            assert_double_eq(expected[i], state.get_globe_view_projection_matrix()[i]);
        }
        for i in 0..4 {
            let _trace = ScopedTrace(format!("plane {i}"));
            assert_double_eq(plane[i], state.get_globe_clipping_plane()[i]);
        }
        for i in 0..3 {
            let _trace = ScopedTrace(format!("camera {i}"));
            assert_double_eq(camera[i], state.get_globe_camera_position()[i]);
        }
        let mut round_trip: Mat4 = [0.0; 16];
        matrix::multiply(
            &mut round_trip,
            &state.get_globe_view_projection_matrix(),
            &state.get_inverse_globe_view_projection_matrix(),
        );
        for (i, value) in round_trip.iter().enumerate() {
            let _trace = ScopedTrace(format!("inverse element {i}"));
            assert_near(if i % 5 == 0 { 1.0 } else { 0.0 }, *value, 1e-9);
        }
    };
    check(transform.get_state());
    transform.jump_to(
        &cam()
            .with_center(Some(ll(-30.0, 100.0)))
            .with_zoom(Some(4.0))
            .with_pitch(Some(50.0))
            .with_bearing(Some(-80.0)),
    );
    check(transform.get_state());
    transform.resize(Size::new(1024, 300)).unwrap();
    check(transform.get_state());
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
    let camera = transform.get_state().get_globe_camera_position();
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
        let zoom_before = transform.get_zoom();
        let latitude_before = transform.get_lat_lng(WrapMode::Wrapped).latitude();
        transform.move_by(&offset, &AnimationOptions::default()).unwrap();
        assert_near(expected.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(expected.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
        // The planet keeps its apparent size: zoom follows the latitude.
        assert_near(
            zoom_before + VerticalPerspectiveProjection::zoom_adjustment(latitude_before, expected.latitude()),
            transform.get_zoom(),
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
    transform.jump_to(&cam().with_zoom(Some(4.5)).with_anchor(Some(anchor)));
    assert_ne!(30.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
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
    let mut last_radius = transform.get_state().get_globe_radius_pixels();
    let mut last_center = transform.get_lat_lng(WrapMode::Wrapped);
    for i in 1..=60 {
        transform.ease_to(
            &cam().with_zoom(Some(6.0 - f64::from(i) * 0.1)).with_anchor(Some(anchor)),
            &AnimationOptions::default(),
        );
        if i % 3 == 0 {
            transform.move_by(&sc(-7.0, 4.0), &AnimationOptions::default()).unwrap();
        }
        let radius = transform.get_state().get_globe_radius_pixels();
        let center = transform.get_lat_lng(WrapMode::Wrapped);
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
    jumped.jump_to(&cam().with_zoom(Some(2.0)).with_anchor(Some(anchor)));
    let jump_move = jumped.get_lat_lng(WrapMode::Wrapped).longitude();
    assert!(jump_move > 1.0, "{jump_move}");

    let mut animated = new_transform();
    set_up_globe_camera(&mut animated, &ll(0.0, 0.0), 1.0);
    animated.ease_to(&cam().with_zoom(Some(2.0)).with_anchor(Some(anchor)), &linear_second());
    for frame in 1..=10 {
        run_to(&mut animated, f64::from(frame) / 10.0);
    }
    assert_near(2.0, animated.get_zoom(), 1e-9);
    let animated_move = animated.get_lat_lng(WrapMode::Wrapped).longitude();
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
            cam().with_bearing(Some(60.0)).with_anchor(Some(anchor))
        } else {
            cam().with_zoom(Some(4.0)).with_anchor(Some(anchor))
        };
        let mut jumped = new_transform();
        set_up_globe_camera(&mut jumped, &ll(45.0, 0.0), 3.0);
        jumped.jump_to(&change);
        assert!((jumped.get_lat_lng(WrapMode::Wrapped).latitude() - 45.0).abs() > 0.5);

        let mut animated = new_transform();
        set_up_globe_camera(&mut animated, &ll(45.0, 0.0), 3.0);
        animated.ease_to(&change, &linear_second());
        for frame in 1..=10 {
            run_to(&mut animated, f64::from(frame) / 10.0);
        }
        assert_near(
            jumped.get_lat_lng(WrapMode::Wrapped).latitude(),
            animated.get_lat_lng(WrapMode::Wrapped).latitude(),
            1e-6,
        );
        assert_near(
            jumped.get_lat_lng(WrapMode::Wrapped).longitude(),
            animated.get_lat_lng(WrapMode::Wrapped).longitude(),
            1e-6,
        );
        assert_near(jumped.get_zoom(), animated.get_zoom(), 1e-6);
    }
}

// gtest: GlobeTransform.PolesAreReachableAndZoomFollowsLatitude
#[test]
fn poles_are_reachable_and_zoom_follows_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 0.0, 0.0, 0.0);
    let radius_at_equator = transform.get_state().get_globe_radius_pixels();
    transform.jump_to(&cam().with_center(Some(ll(80.0, 0.0))));
    assert_near(80.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    // Moving the center towards the pole lowers the zoom by log2(cos 80),
    // below the map's minimum zoom of 0, so the planet keeps its size on
    // screen.
    assert_near(VerticalPerspectiveProjection::zoom_adjustment(0.0, 80.0), transform.get_zoom(), 1e-9);
    assert_near(radius_at_equator, transform.get_state().get_globe_radius_pixels(), 1e-6);
    transform.jump_to(&cam().with_center(Some(ll(80.0, 0.0))).with_zoom(Some(3.0)));
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))));
    assert_near(3.0 + VerticalPerspectiveProjection::zoom_adjustment(80.0, 0.0), transform.get_zoom(), 1e-9);

    // Past the Mercator range the center goes as far toward the pole as the
    // zoom it lands on allows, and the zoom follows the edge of the range:
    // the planet keeps its size. A drag over the pole stops where the zoom
    // lets the center go. GL JS: 89 at zoom 1.9907089805355933, then
    // 89.50632407943635.
    let radius_before = transform.get_state().get_globe_radius_pixels();
    transform.jump_to(&cam().with_center(Some(ll(89.0, 0.0))));
    assert_near(89.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(1.9907089805355933, transform.get_zoom(), 1e-6);
    assert_near(radius_before, transform.get_state().get_globe_radius_pixels(), 1e-6);
    transform.move_by(&sc(0.0, 300.0), &AnimationOptions::default()).unwrap();
    assert_near(89.50632407943635, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(radius_before, transform.get_state().get_globe_radius_pixels(), 1e-6);
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
        assert_near(56.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(11.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        for outside in [ll(56.0, -65.0), ll(80.0, 11.0), ll(56.0, 50.0), ll(30.0, 11.0)] {
            transform.jump_to(&cam().with_center(Some(outside)).with_zoom(Some(4.0)));
            let center = transform.get_lat_lng(WrapMode::Wrapped);
            assert!(bounds.contains(&center), "{}, {}", center.latitude(), center.longitude());
        }
        // A center the whole screen fits around stays where it was asked for.
        transform.jump_to(&cam().with_center(Some(ll(55.0, 15.0))).with_zoom(Some(4.0)));
        assert_near(55.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(15.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        if mode == ConstrainMode::Screen {
            // The screen has to fit inside the bounds, so zooming out is
            // stopped short.
            transform.jump_to(&cam().with_center(Some(ll(56.0, 11.0))).with_zoom(Some(1.0)));
            assert!(transform.get_zoom() > 1.0);
            assert!(bounds.contains(&transform.get_lat_lng(WrapMode::Wrapped)));
        }
    }
}

// gtest: GlobeTransform.MinZoomFollowsLatitude
#[test]
fn min_zoom_follows_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 0.0), 2.0, 0.0, 0.0);
    assert_double_eq(0.0, transform.get_state().get_min_zoom());
    assert_double_eq(0.0, transform.get_state().get_min_zoom_at_latitude(0.0));
    assert_near(-1.0, transform.get_state().get_min_zoom_at_latitude(60.0), 1e-12);
    assert_near(deg2rad(80.0).cos().log2(), transform.get_state().get_min_zoom_at_latitude(80.0), 1e-12);

    // Zoom requests clamp to the floor at the requested center, not at the
    // equator's.
    transform.jump_to(&cam().with_center(Some(ll(80.0, 0.0))).with_zoom(Some(-5.0)));
    assert_near(transform.get_state().get_min_zoom_at_latitude(80.0), transform.get_zoom(), 1e-9);
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(-1.0)));
    assert_double_eq(0.0, transform.get_zoom());
    assert_eq!(0, transform.get_state().get_integer_zoom());

    // Integer zoom never wraps below zero.
    transform.jump_to(&cam().with_center(Some(ll(80.0, 0.0))).with_zoom(Some(-2.0)));
    assert_near(-2.0, transform.get_zoom(), 1e-9);
    assert_eq!(0, transform.get_state().get_integer_zoom());

    // Mercator keeps the plain minimum.
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert_double_eq(transform.get_state().get_min_zoom(), transform.get_state().get_min_zoom_at_latitude(80.0));
    transform.jump_to(&cam().with_center(Some(ll(80.0, 0.0))).with_zoom(Some(-2.0)));
    assert!(transform.get_zoom() >= 0.0);
}

// gtest: GlobeTransform.CameraFollowsTheGlobeBelowTheMinimumScale
#[test]
fn camera_follows_the_globe_below_the_minimum_scale() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(80.0, 0.0), 8.0, 0.0, 0.0);
    transform.set_min_zoom(6.0);
    transform.jump_to(&cam().with_center(Some(ll(80.0, 90.0))).with_zoom(Some(0.0)));
    assert_near(transform.get_state().get_min_zoom_at_latitude(80.0), transform.get_zoom(), 1e-9);
    assert!(transform.get_zoom() < 6.0);
    assert!(transform.get_state().valid());
    let position = transform.get_free_camera_options().position.unwrap();
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
    transform.ease_to(&cam().with_center(Some(ll(40.0, 0.0))), &linear_second());
    run_to(&mut transform, 1.0);
    assert!(!transform.in_transition());
    assert_near(40.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.61549999962236379, transform.get_zoom(), 1e-9);
}

// gtest: GlobeTransform.FlyAdjustsTheZoomToTheNewLatitude
#[test]
fn fly_adjusts_the_zoom_to_the_new_latitude() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 1.0);
    transform.fly_to(&cam().with_center(Some(ll(40.0, 0.0))), &linear_second(), false);
    run_to(&mut transform, 0.5);
    assert_near(20.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.87493158146277261, transform.get_zoom(), 1e-9);
    run_to(&mut transform, 1.0);
    assert!(!transform.in_transition());
    assert_near(40.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.61549999962236379, transform.get_zoom(), 1e-9);
}

// gtest: GlobeTransform.EaseKeepsThePlanetSizeAcrossLatitudes
#[test]
fn ease_keeps_the_planet_size_across_latitudes() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    let radius = transform.get_state().get_globe_radius_pixels();
    transform.ease_to(&cam().with_center(Some(ll(60.0, 0.0))), &linear_second());
    run_to(&mut transform, 0.5);
    assert_near(30.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(0.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(2.7924812503605780, transform.get_zoom(), 1e-9);
    assert_near(radius, transform.get_state().get_globe_radius_pixels(), 1e-6);
    run_to(&mut transform, 1.0);
    assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.0, transform.get_zoom(), 1e-9);
    assert_near(radius, transform.get_state().get_globe_radius_pixels(), 1e-6);
}

// gtest: GlobeTransform.EaseFollowsTheLongitudeSpeedCurve
#[test]
fn ease_follows_the_longitude_speed_curve() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    transform.ease_to(&cam().with_center(Some(ll(60.0, 60.0))), &linear_second());
    run_to(&mut transform, 0.5);
    assert_near(25.026136930423718, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(30.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.7924812503605780, transform.get_zoom(), 1e-9);
    run_to(&mut transform, 1.0);
    assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(2.0, transform.get_zoom(), 1e-9);
}

// gtest: GlobeTransform.EaseWithZoomSpeedsUpTheCenter
#[test]
fn ease_with_zoom_speeds_up_the_center() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
    transform.ease_to(&cam().with_center(Some(ll(60.0, 0.0))).with_zoom(Some(5.0)), &linear_second());
    run_to(&mut transform, 0.5);
    assert_near(42.426406871192853, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(4.0619751435424885, transform.get_zoom(), 1e-9);
    run_to(&mut transform, 1.0);
    assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(5.0, transform.get_zoom(), 1e-9);
}

// gtest: GlobeTransform.FlyFollowsTheGlobePath
#[test]
fn fly_follows_the_globe_path() {
    {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
        transform.fly_to(&cam().with_center(Some(ll(60.0, 60.0))), &linear_second(), false);
        run_to(&mut transform, 0.5);
        assert_near(25.026136930425196, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(30.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(1.8176209209717085, transform.get_zoom(), 1e-9);
        run_to(&mut transform, 1.0);
        assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-8);
        assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(2.0, transform.get_zoom(), 1e-9);
    }
    {
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 3.0);
        transform.fly_to(&cam().with_center(Some(ll(60.0, 60.0))).with_zoom(Some(6.0)), &linear_second(), false);
        run_to(&mut transform, 0.5);
        assert_near(54.663652625664554, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(56.470588235293114, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(2.4045752372957860, transform.get_zoom(), 1e-9);
        run_to(&mut transform, 1.0);
        assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-8);
        assert_near(60.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-8);
        assert_near(6.0, transform.get_zoom(), 1e-9);
    }
}

// gtest: GlobeTransform.FlyStaysAboveTheMinimumZoom
#[test]
fn fly_stays_above_the_minimum_zoom() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 20.0);
    let mut animation = linear_second();
    animation.min_zoom = Some(10.0);
    transform.fly_to(&cam().with_center(Some(ll(0.0, 1.0))).with_zoom(Some(20.0)), &animation, false);
    let mut lowest = 20.0;
    for frame in 1..=10 {
        run_to(&mut transform, f64::from(frame) / 10.0);
        lowest = std_min(lowest, transform.get_zoom());
    }
    // GL JS bottoms out at 9.9999993 on the same frames.
    assert!(lowest >= 10.0 - 1e-5, "{lowest}");
    assert!(lowest <= 10.01, "{lowest}");
    assert_near(20.0, transform.get_zoom(), 1e-9);
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
        flat.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(0.0)));
        let flat_fit = crate::map::camera_for_lat_lngs(&corners, &flat, &EdgeInsets::default()).unwrap();
        assert!(flat_fit.zoom.is_some());
        assert_near(2.4694852833012204, flat_fit.zoom.unwrap(), 1e-7);
    }

    let mut fit = crate::map::camera_for_lat_lngs(&corners, &transform, &EdgeInsets::default()).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717077774070773, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.4964711163134550, fit.zoom.unwrap(), 1e-7);

    fit = crate::map::camera_for_lat_lngs(&corners, &transform, &insets(15.0, 15.0, 15.0, 15.0)).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717077774070773, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.3985253272853888, fit.zoom.unwrap(), 1e-7);

    transform.jump_to(&cam().with_bearing(Some(45.0)));
    fit = crate::map::camera_for_lat_lngs(&corners, &transform, &EdgeInsets::default()).unwrap();
    assert!(fit.center.is_some() && fit.zoom.is_some());
    assert_near(-100.5, fit.center.unwrap().longitude(), 1e-9);
    assert_near(34.717077774070773, fit.center.unwrap().latitude(), 1e-9);
    assert_near(2.4499189488903967, fit.zoom.unwrap(), 1e-7);
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
    let mid_latitude = (bounds.north() + bounds.south()) / 2.0;
    let mid_longitude = (bounds.west() + bounds.east()) / 2.0;
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
        let mut shown = Transform::from_state(transform.get_state().clone());
        shown.jump_to(&fit);
        let state = shown.get_state();
        let left = padding.left() / 512.0 * 2.0 - 1.0;
        let right = (512.0 - padding.right()) / 512.0 * 2.0 - 1.0;
        let top = padding.top() / 512.0 * -2.0 + 1.0;
        let bottom = (512.0 - padding.bottom()) / 512.0 * -2.0 + 1.0;
        let mut nearest = f64::INFINITY;
        for sample in &samples {
            let surface = VerticalPerspectiveProjection::surface_vector(sample);
            let clip = matrix::transform_mat4_value(
                &[surface[0], surface[1], surface[2], 1.0],
                &state.get_globe_view_projection_matrix(),
            );
            let x = clip[0] / clip[3];
            let y = clip[1] / clip[3];
            // std::min({nearest, x - left, right - x, top - y, y - bottom})
            nearest = [x - left, right - x, top - y, y - bottom].into_iter().fold(nearest, std_min);
        }
        assert_near(0.0, nearest, 1e-3);
    }
}

// gtest: TileProjector.MercatorMatchesTheTileMatrix
#[test]
fn mercator_matches_the_tile_matrix() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    transform.jump_to(
        &cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(1.0)).with_bearing(Some(30.0)).with_pitch(Some(20.0)),
    );
    let state = transform.get_state();
    assert!(!state.is_globe_rendering());

    // The bottom-right corner of tile 1/0/0 is lat 0, lng 0: the map center.
    let tile_id = UnwrappedTileID::new(1, 0, 0);
    let projector = TileProjector::new(state, &tile_id);
    let corner = Point::new(f64::from(EXTENT), f64::from(EXTENT));
    let pos =
        matrix::transform_mat4_value(&[corner.x, corner.y, 0.0, 1.0], &state.get_projection_data(&tile_id).main_matrix);

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
    assert_double_eq(1.0, state.get_projection().pixel_scale(state));
}

// gtest: TileProjector.GlobeOccludesTheFarSideAndCorrectsForLatitude
#[test]
fn globe_occludes_the_far_side_and_corrects_for_latitude() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(40.0, 0.0), 1.0, 0.0, 0.0);
    let state = transform.get_state();
    let tile_id = UnwrappedTileID::new(1, 0, 0);
    let projector = TileProjector::new(state, &tile_id);

    // The bottom-right corner (lat 0, lng 0) faces the camera; the bottom-left
    // one (lng -180, lat 0) is behind it.
    assert!(!projector.project(&Point::new(f64::from(EXTENT), f64::from(EXTENT)), 0.0).occluded);
    assert!(projector.project(&Point::new(0.0, f64::from(EXTENT)), 0.0).occluded);

    let cos40 = deg2rad(40.0).cos();
    assert_near(cos40, projector.circle_radius_correction(), 1e-12);
    assert_near(1.0 / cos40, state.get_projection().pixel_scale(state), 1e-12);
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
            latitude: 52.499167,
            transition: 0.5,
            circle: 0.8043864815927202,
            pixel: 1.321324254256826,
            pitched: 1.2637600088069,
        },
        Case { latitude: 60.0, transition: 0.25, circle: 0.875, pixel: 1.25, pitched: 1.0636473098322572 },
        Case { latitude: 60.0, transition: 1.0, circle: 0.5, pixel: 2.0, pitched: 1.2545892393290283 },
        Case { latitude: 0.0, transition: 0.5, circle: 1.0, pixel: 1.0, pitched: 1.7545892393290279 },
    ] {
        let mut transform = new_transform();
        transform.resize(Size::new(256, 256)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::between(
            ProjectionType::Mercator,
            ProjectionType::VerticalPerspective,
            c.transition,
        ));
        transform.jump_to(&cam().with_center(Some(ll(c.latitude, 13.418056))).with_zoom(Some(11.5)));
        let state = transform.get_state();
        let projection = state.get_projection();
        assert_near(c.circle, projection.circle_radius_correction(state), 1e-9);
        assert_near(c.pixel, projection.pixel_scale(state), 1e-9);
        // An anchor a quarter of the way down the zoom 0 tile, at 66.5 degrees
        // north.
        assert_near(
            c.pitched,
            projection.pitched_text_correction(state, &Point::new(4096.0, 2048.0), &UnwrappedTileID::new(0, 0, 0)),
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
        View { center: ll(20.0, 30.0), zoom: 1.0, pitch: 50.0, factor: 1.659065925812732 },
        View { center: ll(0.0, 0.0), zoom: 1.0, pitch: 60.0, factor: 2.3660254037842425 },
        View { center: ll(20.0, 30.0), zoom: 2.0, pitch: 50.0, factor: 1.6590659258122578 },
    ] {
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform
            .jump_to(&cam().with_center(Some(view.center)).with_zoom(Some(view.zoom)).with_pitch(Some(view.pitch)));
        assert_near(view.factor, f64::from(transform.get_state().max_pitch_scale_factor()), 1e-4);
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
    transform.jump_to(&cam().with_center(Some(ll(0.0, -179.0))));
    assert_near(181.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(-179.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    transform.move_by(&sc(-40.0, 0.0), &AnimationOptions::default()).unwrap();
    assert!(transform.get_lat_lng(WrapMode::Unwrapped).longitude() > 181.0);
    transform.jump_to(&cam().with_center(Some(ll(0.0, 179.0))));
    assert_near(179.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);

    // Mercator keeps its wrapped center.
    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    transform.jump_to(&cam().with_center(Some(ll(0.0, -179.0))));
    assert_near(-179.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
}

// Leaving the globe applies Mercator's limits again: its lowest zoom for the
// view's height and a wrapped center.
// gtest: Transform.LeavingTheGlobeConstrainsTheCamera
#[test]
fn leaving_the_globe_constrains_the_camera() {
    let mut transform = new_transform();
    transform.resize(Size::new(400, 800)).unwrap();
    transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(0.3)));
    transform.jump_to(&cam().with_center(Some(ll(0.0, 120.0))));
    transform.jump_to(&cam().with_center(Some(ll(0.0, -150.0))));
    assert_near(210.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(0.3, transform.get_zoom(), 1e-9);

    transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
    assert_near((800.0f64 / 512.0).log2(), transform.get_zoom(), 1e-9);
    assert_near(-150.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
}

// Bounds across the antimeridian keep a globe center past 180 degrees, which
// the globe hands over wrapped.
// gtest: Transform.GlobeBoundsAcrossTheAntimeridian
#[test]
fn globe_bounds_across_the_antimeridian() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 150.0), 3.0, 0.0, 0.0);
    transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 120.0), ll(10.0, 240.0))).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 175.0))));
    transform.jump_to(&cam().with_center(Some(ll(0.0, -175.0))));
    assert_near(185.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);

    // A drag east across the antimeridian.
    transform.jump_to(&cam().with_center(Some(ll(0.0, 179.0))));
    transform.move_by(&sc(-100.0, 0.0), &AnimationOptions::default()).unwrap();
    assert!(transform.get_lat_lng(WrapMode::Unwrapped).longitude() > 180.0);
    assert!(transform.get_lat_lng(WrapMode::Unwrapped).longitude() < 240.0);
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
        transform.jump_to(&cam().with_center(Some(ll(0.0, 180.0))).with_zoom(Some(5.0)));
        transform.set_lat_lng_bounds(LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0))).unwrap();
        transform.jump_to(&cam().with_center(Some(ll(0.0, 185.0))));
        assert_near(185.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
        let camera = transform.get_camera_options(None);
        transform.jump_to(&camera);
        assert_near(185.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
        if globe {
            transform.set_projection_definition(&ProjectionDefinition::from_name("mercator"));
            assert_near(185.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
        }
    }
}

// gtest: Transform.GlobeCameraCenterIsWrapped
#[test]
fn globe_camera_center_is_wrapped() {
    let mut transform = new_transform();
    set_up_globe(&mut transform, &ll(0.0, 179.0), 1.0, 0.0, 0.0);
    transform.jump_to(&cam().with_center(Some(ll(0.0, -179.0))));
    assert_near(181.0, transform.get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
    assert_near(-179.0, transform.get_camera_options(None).center.unwrap().longitude(), 1e-9);
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
            expected: ll(29.64674667013475, -26.3247433402695),
        },
        Pan {
            center: ll(-45.0, 170.0),
            zoom: 8.0,
            bearing: -100.0,
            offset: sc(-50.0, 25.0),
            expected: ll(-44.91280009391755, 169.90853166980656),
        },
        Pan {
            center: ll(70.0, 0.0),
            zoom: 5.0,
            bearing: 0.0,
            offset: sc(200.0, 0.0),
            expected: ll(70.0, -4.23757815424085),
        },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(pan.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &pan.center, pan.zoom, pan.bearing, 0.0);
        transform.move_by(&pan.offset, &linear_second()).unwrap();
        transform.update_transitions(transform.get_latest_transition_start() + milliseconds(1000));
        assert_near(pan.expected.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
        assert_near(pan.expected.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(
            pan.zoom + VerticalPerspectiveProjection::zoom_adjustment(pan.center.latitude(), pan.expected.latitude()),
            transform.get_zoom(),
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
        Fling { zoom: 1.5, halves: [(75.0, 0.5500156865235035), (90.0, -1.0350518437681169)] },
        Fling { zoom: 3.0, halves: [(73.2171601091423, 2.2078682014999638), (86.43432021828463, 0.4649481562318831)] },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(fling.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &ll(60.0, 10.0), fling.zoom, 0.0, 0.0);
        transform.move_by(&sc(0.0, 3000.0), &linear_second()).unwrap();
        let mut latitude = 60.0;
        for frame in 1..=20 {
            transform.update_transitions(transform.get_latest_transition_start() + milliseconds(50 * frame));
            assert_near(10.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
            assert!(transform.get_lat_lng(WrapMode::Wrapped).latitude() >= latitude - 1e-9, "frame {frame}");
            latitude = transform.get_lat_lng(WrapMode::Wrapped).latitude();
            if frame % 10 == 0 {
                let (expected_latitude, expected_zoom) = fling.halves[(frame / 10 - 1) as usize];
                assert_near(expected_latitude, latitude, 1e-6);
                assert_near(expected_zoom, transform.get_zoom(), 1e-6);
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
    transform.move_by(&sc(-5000.0, 0.0), &linear_second()).unwrap();
    let mut longitude = 0.0;
    for frame in 1..=20 {
        transform.update_transitions(transform.get_latest_transition_start() + milliseconds(50 * frame));
        assert!(transform.get_lat_lng(WrapMode::Unwrapped).longitude() >= longitude - 1e-9, "frame {frame}");
        longitude = transform.get_lat_lng(WrapMode::Unwrapped).longitude();
    }
    assert_near(179.5, longitude, 1e-9);
    assert_near(0.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
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
            (19.96528220871278, 6.48152888184427),
        ),
        (
            "center-drag-east 2",
            (19.93062984381388, 2.96383097806563),
            1.0006345588283656,
            0.0,
            0.0,
            (330.0, 240.0),
            (10.0, 0.0),
            (19.89604275335306, -0.5530967909885476),
        ),
        (
            "center-drag-east 4",
            (19.86152078591744, -4.06925749079096),
            1.0012643498868492,
            0.0,
            0.0,
            (330.0, 240.0),
            (10.0, 0.0),
            (19.8270637906032, -7.584654172731689),
        ),
        (
            "off-center-drag 2",
            (16.074405186537916, 4.212860800653175),
            1.0322017912167816,
            0.0,
            0.0,
            (400.0, 180.0),
            (6.0, -4.0),
            (14.115311037321156, 1.3609496277557582),
        ),
        (
            "off-center-drag 4",
            (12.158623543717447, -1.4683004008273883),
            1.057008507859097,
            0.0,
            0.0,
            (400.0, 180.0),
            (6.0, -4.0),
            (10.204304619531165, -4.278521558340458),
        ),
        (
            "drag-north-into-pole 0",
            (70.0, -100.0),
            1.0,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (70.73728720083854, -100.00000000000205),
        ),
        (
            "drag-north-into-pole 20",
            (84.74574401678998, -100.00000000000068),
            -0.9010463352914224,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (85.48303121762915, -100.00000000000068),
        ),
        (
            "drag-north-into-pole 21",
            (85.48303121762915, -100.00000000000068),
            -0.9872050442266238,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (86.22031841846909, -100.00000000000068),
        ),
        (
            "drag-north-into-pole 27",
            (89.90675442266692, -100.00000000000068),
            -0.9872050442266238,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (90.0, -100.00000000000068),
        ),
        (
            "drag-north-into-pole 28",
            (90.0, -100.00000000000068),
            -0.9872050442266238,
            0.0,
            0.0,
            (320.0, 300.0),
            (0.0, 6.0),
            (90.0, -100.00000000000068),
        ),
        (
            "side-drag-at-pole 0",
            (85.0511287798066, -100.0),
            1.0,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (85.0477656113963, -101.87137817590371),
        ),
        (
            "side-drag-at-pole 30",
            (84.95121303919578, -155.737980622539),
            1.0287640984336475,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (84.94791671962916, -157.58177081818087),
        ),
        (
            "side-drag-at-pole 59",
            (84.85648183647973, 151.15114974424728),
            1.0555120380074723,
            0.0,
            0.0,
            (380.0, 240.0),
            (6.0, 0.0),
            (84.85324650692623, 149.33280710271083),
        ),
        (
            "side-drag-centred-pole 0",
            (89.0, 0.0),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (89.34438829057913, 13.508337366700857),
        ),
        (
            "side-drag-centred-pole 2",
            (89.68877639444831, 22.57677926378733),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 28.803865724291313),
        ),
        (
            "side-drag-centred-pole 15",
            (90.0, 83.80781405685116),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 88.39147641789782),
        ),
        (
            "side-drag-centred-pole 29",
            (90.0, 147.97908711150444),
            0.5,
            0.0,
            0.0,
            (340.0, 200.0),
            (0.0, 8.0),
            (90.0, 152.5627494725511),
        ),
        (
            "drag-off-globe 0",
            (10.0, 5.0),
            1.0,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.982236219514714, 1.5494610302675937),
        ),
        (
            "drag-off-globe 2",
            (9.964504640751409, -1.9008899375490955),
            1.0001573094988878,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.946805201839899, -5.3510535945838456),
        ),
        (
            "drag-off-globe 4",
            (9.929137841049005, -8.801030629313573),
            1.0003134815524495,
            0.0,
            0.0,
            (620.0, 240.0),
            (20.0, 0.0),
            (9.911502496783827, -12.250821727578682),
        ),
        (
            "drag-bearing-pitch 0",
            (40.0, 30.0),
            2.5,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.100686468998106, 30.988488451527928),
        ),
        (
            "drag-bearing-pitch 4",
            (40.40265566873954, 33.96278116325789),
            2.4914316029001595,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.50328185738783, 34.95717865523261),
        ),
        (
            "drag-bearing-pitch 7",
            (40.70448865902955, 36.95046737255302),
            2.4849281889924244,
            30.0,
            40.0,
            (300.0, 330.0),
            (-7.0, 5.0),
            (40.80506918481425, 37.94937335237137),
        ),
        (
            "drag-across-antimeridian 0",
            (0.0, 175.0),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, 177.9946248399193),
        ),
        (
            "drag-across-antimeridian 5",
            (0.0, -170.02687580040782),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, -167.0322509604884),
        ),
        (
            "drag-across-antimeridian 9",
            (0.0, -158.0483764407433),
            1.5,
            0.0,
            0.0,
            (300.0, 240.0),
            (-12.0, 0.0),
            (0.0, -155.0537516008231),
        ),
        (
            "drag-south-pole 0",
            (-80.0, 40.0),
            0.8,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-80.49305711248556, 40.00000000000148),
        ),
        (
            "drag-south-pole 10",
            (-84.93057112485239, 40.00000000000193),
            -0.17465574390064653,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-85.42362823733717, 40.00000000000193),
        ),
        (
            "drag-south-pole 19",
            (-89.36808513722191, 40.00000000000193),
            -0.20929101946440645,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-89.86114224970903, 40.00000000000193),
        ),
        (
            "drag-south-pole 20",
            (-89.86114224970903, 40.00000000000193),
            -0.20929101946440645,
            0.0,
            0.0,
            (320.0, 200.0),
            (0.0, -7.0),
            (-90.0, 40.00000000000193),
        ),
    ];
    for (name, center, zoom, bearing, pitch, point, pan_delta, expected) in steps {
        let _trace = ScopedTrace(name.to_owned());
        let mut transform = new_transform();
        transform.resize(Size::new(640, 480)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(
            &cam()
                .with_center(Some(ll(center.0, center.1)))
                .with_zoom(Some(zoom))
                .with_bearing(Some(bearing))
                .with_pitch(Some(pitch)),
        );
        let center = VerticalPerspectiveProjection::drag_center(
            transform.get_state(),
            &sc(point.0, 480.0 - point.1),
            &sc(pan_delta.0, -pan_delta.1),
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
    transform.jump_to(&cam().with_center(Some(ll(LATITUDE_MAX, -100.0))).with_zoom(Some(1.0)));
    for _ in 0..60 {
        transform.move_by_with_anchor(&sc(6.0, 0.0), Some(sc(380.0, 240.0)), &AnimationOptions::default()).unwrap();
    }
    // GL JS after the same 60 steps, run in doubles as in
    // DragCenterMatchesGLJS.
    assert_near(84.85324650692623, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(
        0.0,
        VerticalPerspectiveProjection::difference_of_angles_degrees(
            149.33280710271083,
            transform.get_lat_lng(WrapMode::Wrapped).longitude(),
        ),
        1e-6,
    );
    assert_near(1.0564167839713543, transform.get_zoom(), 1e-6);
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
    transform.jump_to(&cam().with_center(Some(ll(70.0, -100.0))).with_zoom(Some(1.0)));
    for _ in 0..40 {
        transform.move_by_with_anchor(&sc(0.0, 6.0), Some(sc(320.0, 300.0)), &AnimationOptions::default()).unwrap();
    }
    assert_eq!(90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_near(-100.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
    assert_near(-0.9872050442266238, transform.get_zoom(), 1e-6);
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
            (19.964831035703163, 6.458738472277901),
            -1.2105018410808273,
        ),
        (
            "mid-globe-east 7",
            (18.019206354761504, -16.65450732839554),
            1.0171849312490204,
            -8.825955295711765,
            0.0,
            (410.0, 240.0),
            (10.0, 0.0),
            (17.29289010262656, -21.199373456415287),
            -10.205107464700406,
        ),
        (
            "over-north-pole 13",
            (79.41551935272437, -99.99999999999989),
            0.10315332639360152,
            3.410605131648481e-13,
            0.0,
            (320.0, 284.0),
            (0.0, 6.0),
            (80.14639071459133, -99.99999999999807),
            2.0463630789890885e-12,
        ),
        (
            "over-north-pole 25",
            (88.49295346569899, -99.99999999998397),
            -0.9872050442266241,
            1.671196514507756e-11,
            0.0,
            (320.0, 356.0),
            (0.0, 6.0),
            (89.29419866472097, -99.99999999996294),
            3.8198777474462986e-11,
        ),
        (
            "over-north-pole 26",
            (89.29419866472097, -99.99999999996294),
            -0.9872050442266241,
            3.8198777474462986e-11,
            0.0,
            (320.0, 362.0),
            (0.0, 6.0),
            (89.89453036004743, 79.99999999974375),
            179.99999999974605,
        ),
        (
            "over-north-pole 27",
            (89.89453036004743, 79.99999999974375),
            -0.9872050442266241,
            179.99999999974605,
            0.0,
            (320.0, 368.0),
            (0.0, 6.0),
            (89.07238060450973, 79.99999999996999),
            179.99999999997203,
        ),
        (
            "over-north-pole 39",
            (78.96870340062405, 79.99999999999284),
            0.16209611242931793,
            179.999999999995,
            0.0,
            (320.0, 440.0),
            (0.0, 6.0),
            (77.9082916607387, 79.99999999999159),
            179.99999999999363,
        ),
        (
            "across-pole-on-globe 19",
            (89.340072693097, -100.00000000001774),
            -0.009291019464406594,
            -1.6825651982799172e-11,
            0.0,
            (320.0, 320.0),
            (0.0, 8.0),
            (89.83806502107838, -100.00000000007714),
            -7.651124178664759e-11,
        ),
        (
            "across-pole-on-globe 20",
            (89.83806502107838, -100.00000000007714),
            -0.009291019464406594,
            -7.651124178664759e-11,
            0.0,
            (320.0, 328.0),
            (0.0, 8.0),
            (89.6617557958742, 80.00000000003092),
            -179.99999999996658,
        ),
        (
            "across-pole-on-globe 21",
            (89.6617557958742, 80.00000000003092),
            -0.009291019464406594,
            -179.99999999996658,
            0.0,
            (320.0, 336.0),
            (0.0, 8.0),
            (89.15914508699329, 80.00000000001307),
            -179.99999999998465,
        ),
        (
            "beside-north-pole 0",
            (85.05, -100.0),
            1.0,
            0.0,
            0.0,
            (386.0, 240.0),
            (6.0, 0.0),
            (85.04662278483669, -102.12115815315576),
            -2.113243435839649,
        ),
        (
            "beside-north-pole 29",
            (82.61847217815794, -148.0453022994218),
            1.5742929546363558,
            -47.80814125537427,
            0.0,
            (560.0, 240.0),
            (6.0, 0.0),
            (82.46976369277144, -149.0658211264905),
            -48.82003297275798,
        ),
        (
            "off-globe-and-back 5",
            (59.63182334571161, 0.0),
            0.015938893735801596,
            0.0,
            0.0,
            (320.0, 240.0),
            (0.0, 25.0),
            (68.48553614142399, 0.0),
            0.0,
        ),
        (
            "off-globe-and-back 9",
            (84.95332547144974, 180.0),
            -2.506889874048772,
            180.0,
            0.0,
            (320.0, 240.0),
            (0.0, 25.0),
            (76.09961267574481, 180.0),
            180.0,
        ),
        (
            "off-globe-and-back 15",
            (84.95332547144743, 180.0),
            -2.5068898740481123,
            180.0,
            0.0,
            (320.0, 240.0),
            (0.0, -25.0),
            (86.19296173284701, 0.0),
            0.0,
        ),
        (
            "off-globe-and-back 20",
            (33.13938452686452, 0.0),
            0.7438908494163972,
            0.0,
            0.0,
            (320.0, 315.0),
            (0.0, -25.0),
            (20.561601743914935, 0.0),
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
            (39.90522931489954, 27.677367870127796),
            28.54888958514232,
        ),
        (
            "bearing-pitch 9",
            (36.75658194038969, 8.665623636743362),
            1.5647074495325435,
            17.020204416245633,
            20.0,
            (380.0, 210.0),
            (8.0, -5.0),
            (36.00738235824049, 6.187079071036919),
            15.610527473047796,
        ),
    ];
    for (name, center, zoom, bearing, pitch, anchor, pan_delta, expected, expected_bearing) in steps {
        let _trace = ScopedTrace(name.to_owned());
        let mut transform = new_transform();
        transform.resize(Size::new(640, 480)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(
            &cam()
                .with_center(Some(ll(center.0, center.1)))
                .with_zoom(Some(zoom))
                .with_bearing(Some(bearing))
                .with_pitch(Some(pitch))
                .with_fov(Some(45.0)),
        );
        let drag = VerticalPerspectiveProjection::drag_camera(
            transform.get_state(),
            &sc(anchor.0, 480.0 - anchor.1),
            &sc(pan_delta.0, -pan_delta.1),
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
            max_slip = std_max(max_slip, (point.x - cursor.x).hypot(point.y - cursor.y));
        }
        if free_bearing {
            assert!(max_slip < 1e-6, "{max_slip}");
            assert_near(80.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(180.0, mln_math::rad2deg(transform.get_bearing()).abs(), 1e-6);
        } else {
            assert!(max_slip > 4.0, "{max_slip}");
            assert_eq!(90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
        }
    }
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
        transform.jump_to(&cam().with_zoom(Some(1.6)).with_anchor(Some(anchor)));
        let point = transform.lat_lng_to_screen_coordinate(&location);
        let beside_point = transform.lat_lng_to_screen_coordinate(&beside);
        let turn = mln_math::rad2deg((beside_point.y - point.y).atan2(beside_point.x - point.x));
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
        let start = VerticalPerspectiveProjection::surface_vector(&transform.get_lat_lng(WrapMode::Wrapped));
        transform.move_by(&sc(0.0, 400.0), &linear_second()).unwrap();
        let mut previous = 0.0;
        for frame in 1..=20 {
            transform.update_transitions(transform.get_latest_transition_start() + milliseconds(50 * frame));
            let center = VerticalPerspectiveProjection::surface_vector(&transform.get_lat_lng(WrapMode::Wrapped));
            let distance =
                std_clamp(start[0] * center[0] + start[1] * center[1] + start[2] * center[2], -1.0, 1.0).acos();
            assert!(distance >= previous - 1e-9, "frame {frame}");
            previous = distance;
        }
        if free_bearing {
            assert!(transform.get_lat_lng(WrapMode::Wrapped).latitude() < 85.0);
            assert_near(80.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(180.0, mln_math::rad2deg(transform.get_bearing()).abs(), 1e-6);
        } else {
            assert_eq!(90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
            assert_near(0.0, mln_math::rad2deg(transform.get_bearing()), 1e-9);
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
    transform.jump_to(&cam().with_center(Some(ll(90.0, 0.0))).with_zoom(Some(0.0)));
    assert_eq!(90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_near(0.0, transform.get_zoom(), 1e-9);

    set_up_globe_camera(&mut transform, &ll(0.0, 0.0), 0.0);
    transform.jump_to(&cam().with_center(Some(ll(90.0, 0.0))).with_zoom(Some(2.0)));
    assert_near(88.57750940365021, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);

    for (zoom, latitude) in [
        (1.0, 90.0),
        (1.5, 90.0),
        (2.0, 88.57750940365021),
        (2.5, 87.06149291090357),
        (3.0, 85.98950736849177),
        (4.0, LATITUDE_MAX),
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(zoom)));
        set_up_globe_camera(&mut transform, &ll(90.0, 0.0), 0.5);
        transform.jump_to(&cam().with_zoom(Some(zoom)));
        assert_near(latitude, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(zoom, transform.get_zoom(), 1e-9);
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
        89.7739138279545,
        89.34717269436429,
        88.9490091378788,
        88.57750940365021,
        88.23088789526778,
        87.90747859237072,
        87.60572704299784,
        87.3241828921856,
        87.06149291090357,
        86.81639449182124,
        86.58770958064392,
        86.37433901384881,
        86.17525723560607,
    ];
    for longitude in [30.0, 120.0] {
        let _trace = ScopedTrace(format!("longitude {}", std_to_string(longitude)));
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ll(90.0, longitude), 1.0);
        let mut zoom = 1.0;
        for latitude in gljs_latitudes {
            zoom += 0.1;
            let _zoom_trace = ScopedTrace(format!("zoom {}", std_to_string(zoom)));
            transform.jump_to(&cam().with_zoom(Some(zoom)).with_anchor(Some(sc(256.0, 256.0))));
            assert_near(longitude, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
            assert_near(latitude, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
        }
    }

    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(88.0, 30.0), 1.5);
    let pole = transform.lat_lng_to_screen_coordinate(&ll(90.0, 0.0));
    transform.jump_to(&cam().with_zoom(Some(1.7)).with_anchor(Some(pole)));
    assert_near(88.0, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(30.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(149.15333659991416, transform.lat_lng_to_screen_coordinate(&ll(90.0, 0.0)).y, 1e-4);
}

// Without a zoom, GL JS's jump bounds a center past the Mercator range by the
// zoom it lands on, and its ease and flight bound it by the zoom they start
// from (GL JS's camera at 512 by 512).
// gtest: GlobeTransform.JumpBoundsThePoleByTheZoomItLandsOn
#[test]
fn jump_bounds_the_pole_by_the_zoom_it_lands_on() {
    let landing_zoom = -0.5129658685303338;
    let mut jump = new_transform();
    set_up_globe_camera(&mut jump, &ll(10.0, 0.0), 3.0);
    jump.jump_to(&cam().with_center(Some(ll(89.0, 30.0))));
    assert_near(89.0, jump.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-9);
    assert_near(30.0, jump.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(landing_zoom, jump.get_zoom(), 1e-6);

    let mut ease = new_transform();
    set_up_globe_camera(&mut ease, &ll(10.0, 0.0), 3.0);
    ease.ease_to(&cam().with_center(Some(ll(89.0, 30.0))), &linear_second());
    run_to(&mut ease, 0.5);
    assert_near(47.99475368424589, ease.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(7.785022180215378, ease.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
    assert_near(2.442592415258865, ease.get_zoom(), 1e-6);
    run_to(&mut ease, 1.0);
    assert_near(85.98950736849177, ease.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(30.0, ease.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
    assert_near(landing_zoom, ease.get_zoom(), 1e-6);

    let mut fly = new_transform();
    set_up_globe_camera(&mut fly, &ll(10.0, 0.0), 3.0);
    fly.fly_to(&cam().with_center(Some(ll(89.0, 30.0))), &linear_second(), false);
    run_to(&mut fly, 1.0);
    assert_near(85.98950736849132, fly.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
    assert_near(landing_zoom, fly.get_zoom(), 1e-6);
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
            camera: cam().with_center(Some(ll(90.0, 20.0))).with_zoom(Some(0.0)),
            halves: [
                Frame { latitude: 78.05612639366316, longitude: 20.0, zoom: 0.9949038280649232 },
                Frame { latitude: 90.0, longitude: 20.0, zoom: 0.0 },
            ],
        },
        Ease {
            name: "onto the pole without a zoom",
            start: ll(60.0, 20.0),
            start_zoom: 1.0,
            camera: cam().with_center(Some(ll(89.5, 50.0))),
            halves: [
                Frame { latitude: 74.75, longitude: 29.139640453666175, zoom: 0.07330592239273392 },
                Frame { latitude: 89.5, longitude: 50.0, zoom: -1.5350518437681169 },
            ],
        },
        Ease {
            name: "off the pole",
            start: ll(90.0, 0.0),
            start_zoom: 0.0,
            camera: cam().with_center(Some(ll(30.0, 40.0))).with_zoom(Some(1.0)),
            halves: [
                Frame { latitude: 68.78679656440357, longitude: 24.852188049491588, zoom: 0.9047067439922991 },
                Frame { latitude: 30.0, longitude: 40.0, zoom: 1.0 },
            ],
        },
        Ease {
            name: "zooming in at the pole",
            start: ll(90.0, 0.0),
            start_zoom: 0.5,
            camera: cam().with_zoom(Some(4.0)),
            halves: [
                Frame { latitude: 86.50061975668255, longitude: 0.0, zoom: 2.25 },
                Frame { latitude: LATITUDE_MAX, longitude: 0.0, zoom: 4.0 },
            ],
        },
    ] {
        let _trace = ScopedTrace(ease.name.to_owned());
        let mut transform = new_transform();
        set_up_globe_camera(&mut transform, &ease.start, ease.start_zoom);
        transform.ease_to(&ease.camera, &linear_second());
        for half in 1..=2 {
            run_to(&mut transform, f64::from(half) / 2.0);
            let expected = &ease.halves[(half - 1) as usize];
            assert_near(expected.latitude, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
            assert_near(expected.longitude, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
            assert_near(expected.zoom, transform.get_zoom(), 1e-6);
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
    transform.fly_to(&cam().with_center(Some(ll(90.0, 0.0))).with_zoom(Some(0.0)), &linear_second(), false);
    run_to(&mut transform, 1.0);
    assert_eq!(90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
    assert_near(0.0, transform.get_zoom(), 1e-9);

    set_up_globe_camera(&mut transform, &ll(60.0, 20.0), 2.0);
    transform.fly_to(&cam().with_center(Some(ll(89.9, 20.0))).with_zoom(Some(0.0)), &linear_second(), false);
    for (k, latitude, zoom) in [(0.5, 77.69090627898427, 0.6859841910974791), (1.0, 89.9, 0.0)] {
        let _trace = ScopedTrace(format!("k {}", std_to_string(k)));
        run_to(&mut transform, k);
        assert_near(latitude, transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(20.0, transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-9);
        assert_near(zoom, transform.get_zoom(), 1e-6);
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
            expected: ll(88.2247980153879, 39.898924941760924),
        },
        Zoom {
            center: ll(90.0, 0.0),
            zoom: 0.5,
            zoom_delta: 2.0,
            anchor: sc(400.0, 300.0),
            expected: ll(87.69055327579845, 0.0),
        },
    ] {
        let _trace = ScopedTrace(format!("zoom {}", std_to_string(zoom.zoom)));
        let mut transform = new_transform();
        set_up_globe(&mut transform, &zoom.center, zoom.zoom, 0.0, 0.0);
        transform.jump_to(&cam().with_zoom(Some(zoom.zoom + zoom.zoom_delta)).with_anchor(Some(zoom.anchor)));
        assert_near(zoom.expected.latitude(), transform.get_lat_lng(WrapMode::Wrapped).latitude(), 1e-6);
        assert_near(zoom.expected.longitude(), transform.get_lat_lng(WrapMode::Wrapped).longitude(), 1e-6);
        assert_near(zoom.zoom + zoom.zoom_delta, transform.get_zoom(), 1e-9);
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
    transform.jump_to(&cam().with_center(Some(ll(0.0, 179.0))).with_zoom(Some(5.0)));
    let degree = 512.0 * 32.0 / 360.0;
    transform.set_gesture_in_progress(true);
    transform.move_by(&sc(-2.0 * degree, 0.0), &AnimationOptions::new(seconds(10))).unwrap();
    transform.update_transitions(transform.get_latest_transition_start());
    assert!(transform.get_lat_lng(WrapMode::Unwrapped).longitude() < -180.0);
    let center = transform.get_camera_options(None).center.unwrap();
    assert_near(transform.get_lat_lng(WrapMode::Unwrapped).longitude(), center.longitude(), 1e-9);
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
    transform.jump_to(&CameraOptions { padding: Some(infinite), ..cam() });
    assert_eq!(transform.get_state().get_edge_insets(), infinite);
    transform.jump_to(&CameraOptions { padding: Some(EdgeInsets::default()), ..cam() });
    assert_eq!(transform.get_state().get_edge_insets(), infinite);
    transform.fly_to(
        &CameraOptions { padding: Some(EdgeInsets::default()), ..cam() },
        &AnimationOptions::default(),
        false,
    );
    assert_eq!(transform.get_state().get_edge_insets(), infinite);
}

// Port-only: a screen coordinate without a LatLng is the C++ LatLng
// constructor's std::domain_error, returned instead of thrown.
#[test]
fn nan_screen_coordinates_are_errors() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let nan = sc(f64::NAN, 500.0);
    let error = transform.screen_coordinate_to_lat_lng(&nan, WrapMode::Wrapped).unwrap_err();
    assert!(error.ends_with("must not be NaN"), "{error}");
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    assert!(transform.move_by(&nan, &AnimationOptions::default()).is_err());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), center);
    // An anchor without a LatLng is ignored (C++ throws).
    transform.jump_to(&CameraOptions { anchor: Some(nan), zoom: Some(2.0), ..cam() });
    assert_eq!(transform.get_zoom(), 2.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), center);
}

// Port-only: the globe returns the errors Mercator does for a NaN point,
// immediate or animated, and ignores a NaN anchor.
#[test]
fn nan_screen_coordinates_are_errors_on_the_globe() {
    let mut transform = new_transform();
    set_up_globe_camera(&mut transform, &ll(10.0, 10.0), 3.0);
    let nan = sc(f64::NAN, 0.0);
    let error = transform.screen_coordinate_to_lat_lng(&nan, WrapMode::Wrapped).unwrap_err();
    assert!(error.ends_with("must not be NaN"), "{error}");
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    assert!(transform.move_by(&nan, &AnimationOptions::default()).is_err());
    assert!(transform.move_by(&nan, &AnimationOptions::new(seconds(1))).is_err());
    assert!(transform.move_by(&sc(f64::INFINITY, 0.0), &AnimationOptions::default()).is_err());
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), center);
    // A NaN anchor is off the planet: the drag turns it around the center.
    transform.move_by_with_anchor(&sc(10.0, 0.0), Some(nan), &AnimationOptions::default()).unwrap();
    assert_ne!(transform.get_lat_lng(WrapMode::Wrapped), center);
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    transform.jump_to(&CameraOptions { anchor: Some(nan), zoom: Some(2.0), ..cam() });
    assert_eq!(transform.get_zoom(), 2.0);
    assert_eq!(transform.get_lat_lng(WrapMode::Wrapped), center);
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
    transform.jump_to(&CameraOptions { anchor: Some(sc(5.0, 5.0)), zoom: Some(2.0), ..cam() });
    assert_eq!(transform.get_zoom(), 2.0);
}

// The tests below port maplibre-native-ffi's patches 0015 (independent
// camera animations) and 0043 (cancel transitions by id).

const LINEAR: UnitBezier = UnitBezier::new(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0);

/// An end function that counts its calls.
fn counting_end(options: &mut AnimationOptions) -> Rc<Cell<u32>> {
    let count = Rc::new(Cell::new(0));
    let finished = count.clone();
    options.transition_end_fn = Some(Rc::new(move |_: &TransitionEnd| finished.set(finished.get() + 1)));
    count
}

fn bearing_degrees(transform: &Transform) -> f64 {
    mln_math::rad2deg(-transform.get_bearing())
}

// gtest: Transform.IndependentCameraTracks
#[test]
fn independent_camera_tracks() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)).with_center_altitude(Some(120.0)));
    let mut zoom = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(2)) };
    let zoom_finished = counting_end(&mut zoom);
    transform.ease_to(&cam().with_zoom(Some(12.0)), &zoom);
    let zoom_start = transform.get_latest_transition_start();
    transform.update_transitions(zoom_start + milliseconds(500));
    assert_near(transform.get_zoom(), 9.0, 1e-5);

    let mut center = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(8)) };
    let center_finished = counting_end(&mut center);
    transform.ease_to(&cam().with_center(Some(ll(20.0, 30.0))), &center);
    let center_start = transform.get_latest_transition_start();
    assert_eq!(zoom_finished.get(), 0);
    transform.update_transitions(zoom_start + seconds(2));
    assert_near(transform.get_zoom(), 12.0, 1e-5);
    assert!(transform.get_lat_lng(WrapMode::Wrapped).longitude() > 0.0);
    assert!(transform.get_lat_lng(WrapMode::Wrapped).longitude() < 30.0);
    assert_near(transform.get_state().get_center_altitude(), 120.0, 1e-5);
    assert_eq!(zoom_finished.get(), 1);
    assert_eq!(center_finished.get(), 0);
    assert!(transform.is_panning());
    assert!(!transform.is_scaling());
    assert!(transform.in_transition());
    transform.update_transitions(center_start + seconds(8));
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 20.0, 1e-5);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 30.0, 1e-5);
    assert_near(transform.get_zoom(), 12.0, 1e-5);
    assert_eq!(center_finished.get(), 1);
    assert!(!transform.in_transition());
}

// gtest: Transform.PartialReplacementKeepsCommandAndTiming
#[test]
fn partial_replacement_keeps_command_and_timing() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let mut animation = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(4)) };
    let finished = counting_end(&mut animation);
    transform.ease_to(&cam().with_zoom(Some(12.0)).with_bearing(Some(80.0)), &animation);
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + seconds(1));
    assert_near(transform.get_zoom(), 9.0, 1e-5);
    assert_near(bearing_degrees(&transform), 20.0, 1e-5);
    transform.jump_to(&cam().with_zoom(Some(10.0)));
    assert_eq!(finished.get(), 0);
    assert!(transform.is_rotating());
    assert!(!transform.is_scaling());
    transform.update_transitions(start + seconds(2));
    assert_near(transform.get_zoom(), 10.0, 1e-5);
    assert_near(bearing_degrees(&transform), 40.0, 1e-5);
    transform.update_transitions(start + seconds(4));
    assert_eq!(finished.get(), 1);
    assert_near(bearing_degrees(&transform), 80.0, 1e-5);
    transform.cancel_transitions();
    assert_eq!(finished.get(), 1);
}

// gtest: Transform.FlightAndAnchorCoupleCenterWithTheirProperties
#[test]
fn flight_and_anchor_couple_center_with_their_properties() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let mut flight = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(4)) };
    let flight_finished = counting_end(&mut flight);
    transform.fly_to(
        &cam().with_center(Some(ll(20.0, 30.0))).with_zoom(Some(12.0)).with_roll(Some(20.0)),
        &flight,
        false,
    );
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + seconds(1));
    assert_near(mln_math::rad2deg(transform.get_roll()), 5.0, 1e-5);
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    transform.jump_to(&cam().with_zoom(Some(10.0)));
    assert_eq!(flight_finished.get(), 0); // Roll remains after the coupled flight ends.
    transform.update_transitions(start + seconds(4));
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), center.latitude(), 1e-5);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), center.longitude(), 1e-5);
    assert_near(transform.get_zoom(), 10.0, 1e-5);
    assert_near(mln_math::rad2deg(transform.get_roll()), 20.0, 1e-5);
    assert_eq!(flight_finished.get(), 1);

    let mut anchor_animation = AnimationOptions::new(seconds(4));
    let anchor_finished = counting_end(&mut anchor_animation);
    let anchor = sc(200.0, 300.0);
    let fixed = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
    transform.ease_to(&cam().with_zoom(Some(12.0)).with_anchor(Some(anchor)), &anchor_animation);
    transform.update_transitions(transform.get_latest_transition_start() + seconds(1));
    let pixel = transform.lat_lng_to_screen_coordinate(&fixed);
    assert_near(pixel.x, anchor.x, 1e-5);
    assert_near(pixel.y, anchor.y, 1e-5);
    transform.jump_to(&cam().with_bearing(Some(70.0)));
    let rotated_pixel = transform.lat_lng_to_screen_coordinate(&fixed);
    assert_near(rotated_pixel.x, anchor.x, 1e-5);
    assert_near(rotated_pixel.y, anchor.y, 1e-5);
    assert_eq!(anchor_finished.get(), 0);
    transform.jump_to(&cam().with_center(Some(ll(10.0, 15.0))));
    assert_eq!(anchor_finished.get(), 1);
    assert!(!transform.in_transition());
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 10.0, 1e-5);

    transform.ease_to(&cam().with_center(Some(ll(20.0, 30.0))).with_anchor(Some(anchor)), &anchor_animation);
    transform.ease_to(&cam().with_zoom(Some(14.0)), &AnimationOptions::new(seconds(2)));
    assert_eq!(anchor_finished.get(), 1); // Explicit center ignores the anchor.
    transform.cancel_transitions();
    assert_eq!(anchor_finished.get(), 2);
}

// gtest: Transform.ComposePaddingPitchAndFieldOfView
#[test]
fn compose_padding_pitch_and_field_of_view() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let animation = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(4)) };
    transform.ease_to(&cam().with_padding(Some(insets(300.0, 20.0, 0.0, 0.0))), &animation);
    transform.ease_to(&cam().with_pitch(Some(60.0)).with_fov(Some(45.0)), &animation);
    let end = transform.get_latest_transition_start() + seconds(4);
    transform.update_transitions(end);
    assert_eq!(transform.get_state().get_edge_insets(), insets(300.0, 20.0, 0.0, 0.0));
    assert_near(mln_math::rad2deg(transform.get_field_of_view()), 45.0, 1e-5);
    let max_pitch =
        transform.get_state().get_max_pitch() + deg2rad(45.0) / 2.0 - (1.3 * (deg2rad(45.0) / 2.0).tan()).atan();
    assert!(max_pitch < deg2rad(60.0));
    assert_near(transform.get_pitch(), max_pitch, 1e-5);
    assert!(!transform.in_transition());
}

// gtest: Transform.CameraCallbacksCanReplaceAndCancelTransitions has no
// port: its callbacks cancel and start commands on the Transform that is
// calling them, which the Rust callbacks can't borrow (see transform.rs).

#[derive(Default)]
struct EventObserver {
    /// 0 for each did-change notification, and what the tests push.
    events: RefCell<Vec<i32>>,
    changes: Cell<u32>,
}

impl TransformObserver for EventObserver {
    fn on_camera_did_change(&self, _change: CameraChange) {
        self.events.borrow_mut().push(0);
        self.changes.set(self.changes.get() + 1);
    }
}

// gtest: Transform.PartialCameraCommandsFinishTogetherForStillImage
#[test]
fn partial_camera_commands_finish_together_for_still_image() {
    let observer = Rc::new(EventObserver::default());
    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(512, 512)).unwrap();
    let push = |event: i32| {
        let observer = observer.clone();
        Some(Rc::new(move |_: &TransitionEnd| observer.events.borrow_mut().push(event)) as Rc<dyn Fn(&TransitionEnd)>)
    };
    let position = AnimationOptions { transition_end_fn: push(1), ..AnimationOptions::new(seconds(5)) };
    let bearing = AnimationOptions { transition_end_fn: push(2), ..AnimationOptions::new(seconds(10)) };
    transform.ease_to(&cam().with_center(Some(ll(20.0, 30.0))).with_zoom(Some(12.0)), &position);
    transform.ease_to(&cam().with_bearing(Some(60.0)), &bearing);
    observer.events.borrow_mut().clear();
    transform.jump_to(&cam().with_zoom(Some(10.0)));
    assert_eq!(*observer.events.borrow(), [0]);

    // Static maps advance to the maximum time point for a still image. Each
    // command finishes before its own did-change notification, even when
    // another command has replaced one of its properties.
    observer.events.borrow_mut().clear();
    transform.update_transitions(TimePoint::max());
    assert_eq!(*observer.events.borrow(), [1, 0, 2, 0]);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 20.0, 1e-5);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), 30.0, 1e-5);
    assert_near(transform.get_zoom(), 10.0, 1e-5);
    assert_near(bearing_degrees(&transform), 60.0, 1e-5);
    assert!(!transform.in_transition());
    transform.cancel_transitions();
    assert_eq!(*observer.events.borrow(), [1, 0, 2, 0]);
}

// gtest: Transform.CancelTransitionsByIdLeavesOthersRunning
#[test]
fn cancel_transitions_by_id_leaves_others_running() {
    let observer = Rc::new(EventObserver::default());
    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let mut zoom = AnimationOptions { transition_id: Some(1), ..AnimationOptions::new(seconds(4)) };
    let zoom_finished = counting_end(&mut zoom);
    let mut bearing = AnimationOptions { transition_id: Some(2), ..AnimationOptions::new(seconds(4)) };
    let bearing_finished = counting_end(&mut bearing);
    transform.ease_to(&cam().with_zoom(Some(12.0)), &zoom);
    transform.ease_to(&cam().with_bearing(Some(80.0)), &bearing);
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + seconds(1));
    let zoom_at_cancel = transform.get_zoom();
    observer.changes.set(0);

    // Only the command with the identity finishes, once, and the other keeps
    // animating.
    assert!(transform.cancel_transitions_with_id(1));
    assert_eq!(zoom_finished.get(), 1);
    assert_eq!(bearing_finished.get(), 0);
    assert_eq!(observer.changes.get(), 1);
    assert!(transform.in_transition());
    assert!(!transform.is_scaling());
    assert!(transform.is_rotating());

    // An identity that no running command carries finishes nothing and
    // reports no change.
    assert!(!transform.cancel_transitions_with_id(1));
    assert!(!transform.cancel_transitions_with_id(3));
    assert_eq!(zoom_finished.get(), 1);
    assert_eq!(observer.changes.get(), 1);
    assert!(transform.in_transition());

    transform.update_transitions(start + seconds(4));
    assert_near(transform.get_zoom(), zoom_at_cancel, 1e-5);
    assert_near(bearing_degrees(&transform), 80.0, 1e-5);
    assert_eq!(zoom_finished.get(), 1);
    assert_eq!(bearing_finished.get(), 1);
    assert!(!transform.in_transition());
}

// gtest: Transform.FlyDurationRange
/// maplibre-native-ffi's patch throws for a derived duration that Duration
/// can't hold; the port jumps instead, as a flight past `max_duration` does.
#[test]
fn fly_duration_range() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    let camera = cam().with_zoom(Some(1.0));
    for velocity in [f64::from_bits(1), 1e-12] {
        transform.jump_to(&cam().with_zoom(Some(0.0)));
        let (animation, ended) = record_end();
        let animation = AnimationOptions { velocity: Some(velocity), ..animation };
        transform.fly_to(&camera, &animation, false);
        assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
        assert_double_eq(transform.get_zoom(), 1.0);
        assert!(!transform.in_transition());
    }

    transform.jump_to(&cam().with_zoom(Some(0.0)));
    let animation = AnimationOptions { velocity: Some(1e-9), ..AnimationOptions::default() };
    transform.fly_to(&camera, &animation, false);
    assert!(transform.get_latest_transition_duration() > Duration::zero());
    assert!(transform.in_transition());
    transform.cancel_transitions();

    transform.jump_to(&cam().with_zoom(Some(0.0)));
    let animation = AnimationOptions {
        velocity: Some(f64::from_bits(1)),
        duration: Some(Duration::zero()),
        ..AnimationOptions::default()
    };
    transform.fly_to(&camera, &animation, false);
    assert_double_eq(transform.get_zoom(), 1.0);
    assert!(!transform.in_transition());
}

/// GL JS's `maxDuration`: a flight longer than it, by a set or a derived
/// duration, jumps; a shorter one flies as without it; an ease ignores it.
#[test]
fn fly_past_max_duration_jumps() {
    let mut transform = new_transform();
    transform.resize(Size::new(512, 512)).unwrap();
    let target = cam().with_center(Some(ll(40.0, -74.0))).with_zoom(Some(10.0));
    let derived = || {
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.fly_to(&target, &AnimationOptions::default(), false);
        transform.get_latest_transition_duration()
    };
    let natural = derived();
    assert!(natural > seconds(1));

    // Under the cap: the same flight.
    let (animation, ended) = record_end();
    let capped = AnimationOptions { max_duration: Some(natural + milliseconds(1)), ..animation };
    transform.fly_to(&target, &capped, false);
    assert_eq!(transform.get_latest_transition_duration(), natural);
    assert!(transform.in_transition());
    transform.cancel_transitions();
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Cancelled));

    // Over it, derived or set: a jump.
    for animation in [
        AnimationOptions { max_duration: Some(natural - milliseconds(1)), ..AnimationOptions::default() },
        AnimationOptions { max_duration: Some(seconds(1)), ..AnimationOptions::new(seconds(2)) },
    ] {
        transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(0.0)));
        let (end, ended) = record_end();
        transform.fly_to(&target, &AnimationOptions { transition_end_fn: end.transition_end_fn, ..animation }, false);
        assert!(!transform.in_transition());
        assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
        assert_near(transform.get_zoom(), 10.0, 1e-9);
        assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), 40.0, 1e-9);
        assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), -74.0, 1e-9);
    }

    // An ease that reuses the flight keeps its duration.
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(0.0)));
    transform.ease_to(
        &target,
        &AnimationOptions { max_duration: Some(milliseconds(1)), ..AnimationOptions::new(seconds(2)) },
    );
    assert_eq!(transform.get_latest_transition_duration(), seconds(2));
    assert!(transform.in_transition());
}

/// The friction decay rate per second for a half-life.
fn friction_rate(half_life: Duration) -> f64 {
    std::f64::consts::LN_2 / seconds_f64(half_life)
}

fn friction(half_life: Duration, stop_speed: f64) -> MomentumDecay {
    MomentumDecay { half_life, stop_speed, ..MomentumDecay::default() }
}

/// A momentum with the C++ header's defaults, which the ported gtests use:
/// plain friction for every part ([`CameraMomentum::default`] takes
/// maplibre-native-ffi's C API defaults instead).
fn header_momentum() -> CameraMomentum {
    CameraMomentum {
        pan_decay: MomentumDecay::FRICTION,
        zoom_decay: MomentumDecay::FRICTION,
        bearing_decay: MomentumDecay::FRICTION,
        pitch_decay: MomentumDecay::FRICTION,
        ..CameraMomentum::default()
    }
}

/// Animation options whose end function keeps how the command ended.
fn record_end() -> (AnimationOptions, Rc<Cell<Option<TransitionEnd>>>) {
    let ended = Rc::new(Cell::new(None));
    let record = ended.clone();
    let animation = AnimationOptions {
        transition_end_fn: Some(Rc::new(move |end: &TransitionEnd| {
            assert!(record.replace(Some(*end)).is_none(), "a command ends once");
        })),
        ..AnimationOptions::default()
    };
    (animation, ended)
}

fn pitch_degrees(transform: &Transform) -> f64 {
    mln_math::rad2deg(transform.get_pitch())
}

// gtest: Transform.MomentumSlowsEachPartByFriction
#[test]
fn momentum_slows_each_part_by_friction() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(10.0, 20.0))).with_zoom(Some(8.0)).with_pitch(Some(10.0)));
    let center = transform.get_lat_lng(WrapMode::Wrapped);

    let momentum = CameraMomentum {
        zoom_velocity: Some(1.0),
        zoom_decay: friction(milliseconds(250), 0.01),
        bearing_velocity: Some(-10.0),
        pitch_velocity: Some(5.0),
        ..header_momentum()
    };
    let (animation, ended) = record_end();
    transform.start_momentum(&momentum, &animation);
    let start = transform.get_latest_transition_start();
    assert!(transform.in_transition());

    // A speed v that decays at rate k has moved (v / k)(1 - e^(-kt)).
    let zoom_rate = friction_rate(milliseconds(250));
    transform.update_transitions(start + milliseconds(500));
    assert_near(transform.get_zoom(), 8.0 + 1.0 / zoom_rate * -(-zoom_rate * 0.5).exp_m1(), 1e-9);
    assert!(ended.get().is_none());

    // Each part stops at its stop speed, so it moves (v - stop) / k.
    let rate = friction_rate(milliseconds(140));
    transform.update_transitions(start + seconds(3));
    assert_near(transform.get_zoom(), 8.0 + (1.0 - 0.01) / zoom_rate, 1e-9);
    assert_near(bearing_degrees(&transform), -(10.0 - 1.0) / rate, 1e-9);
    assert_near(pitch_degrees(&transform), 10.0 + (5.0 - 1.0) / rate, 1e-9);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).latitude(), center.latitude(), 1e-9);
    assert_near(transform.get_lat_lng(WrapMode::Wrapped).longitude(), center.longitude(), 1e-9);
    let ended = ended.get().expect("the momentum ended");
    assert_eq!(ended.outcome, TransitionOutcome::Completed);
    assert_eq!(ended.velocity.zoom, 0.0);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumPansItsDistanceOnScreen
#[test]
fn momentum_pans_its_distance_on_screen() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(10.0, 20.0))).with_zoom(Some(8.0)));
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    let momentum = CameraMomentum { pan_velocity: Some(sc(1200.0, -500.0)), ..header_momentum() };
    transform.start_momentum(&momentum, &AnimationOptions::default());
    let start = transform.get_latest_transition_start();
    let rate = friction_rate(milliseconds(140));

    transform.update_transitions(start + milliseconds(100));
    let moved = -(-rate * 0.1).exp_m1() / rate;
    let point = transform.lat_lng_to_screen_coordinate(&center);
    assert_near(point.x, 500.0 + 1200.0 * moved, 1e-6);
    assert_near(point.y, 500.0 - 500.0 * moved, 1e-6);

    transform.update_transitions(start + seconds(3));
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
    let start = transform.get_lat_lng(WrapMode::Wrapped);
    let momentum = CameraMomentum { pan_velocity: Some(sc(0.0, 800.0)), ..header_momentum() };
    transform.start_momentum(&momentum, &AnimationOptions::default());
    let begin = transform.get_latest_transition_start();
    transform.update_transitions(begin + seconds(3));
    assert!(!transform.in_transition());
    let end = transform.get_lat_lng(WrapMode::Wrapped);
    assert!(end.latitude() > start.latitude() + 1.0, "{}", end.latitude());
    assert_near(
        3.0 + VerticalPerspectiveProjection::zoom_adjustment(start.latitude(), end.latitude()),
        transform.get_zoom(),
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
        let start = transform.get_zoom();
        assert!(start < 0.0, "{start}");
        let momentum = CameraMomentum { zoom_velocity: Some(zoom_velocity), ..header_momentum() };
        transform.start_momentum(&momentum, &AnimationOptions::default());
        let begin = transform.get_latest_transition_start();
        transform.update_transitions(begin + milliseconds(16));
        let zoom = transform.get_zoom();
        let min_zoom =
            transform.get_state().get_min_zoom_at_latitude(transform.get_lat_lng(WrapMode::Wrapped).latitude());
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
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let momentum = CameraMomentum {
        zoom_velocity: Some(-1.0),
        zoom_decay: MomentumDecay {
            model: MomentumDecayModel::Ease,
            distance: 2.0,
            duration: seconds(1),
            easing: MomentumEasing { x1: 1.0 / 3.0, y1: 1.0 / 3.0, x2: 2.0 / 3.0, y2: 2.0 / 3.0 },
            ..MomentumDecay::default()
        },
        ..header_momentum()
    };
    let (animation, ended) = record_end();
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(250));
    assert_near(transform.get_zoom(), 8.0 - 0.5, 1e-6);
    transform.cancel_transitions();
    assert_near(ended.get().expect("the momentum ended").velocity.zoom, -2.0, 1e-6);

    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(transform.get_latest_transition_start() + seconds(1));
    assert_near(transform.get_zoom(), 8.0 - 0.5 - 2.0, 1e-9);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumKeepsItsAnchorInPlace
#[test]
fn momentum_keeps_its_anchor_in_place() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(
        &cam()
            .with_center(Some(ll(10.0, 20.0)))
            .with_zoom(Some(8.0))
            .with_pitch(Some(20.0))
            .with_center_altitude(Some(120.0)),
    );
    let anchor = sc(800.0, 300.0);
    let ground = transform.screen_coordinate_to_lat_lng(&anchor, WrapMode::Wrapped).unwrap();
    let momentum = CameraMomentum { pitch_velocity: Some(20.0), pitch_anchor: Some(anchor), ..header_momentum() };
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(transform.get_latest_transition_start() + seconds(3));
    assert!(pitch_degrees(&transform) > 20.0);
    let point = transform.lat_lng_to_screen_coordinate(&ground);
    // Restoring the altitude after the correction moves the view by far
    // less than a pixel.
    assert_near(point.x, anchor.x, 1e-4);
    assert_near(point.y, anchor.y, 1e-4);
    assert_near(transform.get_state().get_center_altitude(), 120.0, 1e-6);
}

// gtest: Transform.MomentumCapsItsDistance
#[test]
fn momentum_caps_its_distance() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let momentum = CameraMomentum {
        zoom_velocity: Some(50.0),
        zoom_decay: MomentumDecay { max_distance: 2.5, ..MomentumDecay::default() },
        ..header_momentum()
    };
    transform.start_momentum(&momentum, &AnimationOptions::default());
    transform.update_transitions(transform.get_latest_transition_start() + seconds(3));
    assert_near(transform.get_zoom(), 10.5, 1e-9);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumReportsItsSpeedWhenCancelled
#[test]
fn momentum_reports_its_speed_when_cancelled() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    let momentum = CameraMomentum {
        pan_velocity: Some(sc(1000.0, 0.0)),
        pan_decay: friction(seconds(1), 1.0),
        zoom_velocity: Some(1.0),
        zoom_decay: friction(seconds(1), 0.01),
        ..header_momentum()
    };
    let (animation, ended) = record_end();
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(transform.get_latest_transition_start() + seconds(1));
    transform.cancel_transitions();
    let ended = ended.get().expect("the momentum ended");
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
    transform.jump_to(&cam().with_zoom(Some(8.0)));
    let momentum = CameraMomentum {
        pan_velocity: Some(sc(1000.0, 0.0)),
        pan_decay: friction(seconds(1), 1.0),
        zoom_velocity: Some(1.0),
        zoom_decay: friction(seconds(1), 0.01),
        ..header_momentum()
    };
    let (animation, ended) = record_end();
    transform.start_momentum(&momentum, &animation);
    let start = transform.get_latest_transition_start();
    assert!(transform.is_panning());
    assert!(transform.is_scaling());

    transform.jump_to(&cam().with_zoom(Some(3.0)));
    assert!(ended.get().is_none());
    assert!(transform.is_panning());
    assert!(!transform.is_scaling());
    let before = transform.get_lat_lng(WrapMode::Wrapped);
    transform.update_transitions(start + milliseconds(500));
    assert_near(transform.get_zoom(), 3.0, 1e-9);
    assert_ne!(transform.get_lat_lng(WrapMode::Wrapped).longitude(), before.longitude());

    transform.jump_to(&cam().with_center(Some(ll(10.0, 20.0))));
    assert_eq!(ended.get().expect("the momentum ended").outcome, TransitionOutcome::Replaced);
    assert!(!transform.in_transition());
}

// gtest: Transform.MomentumSettlesOnTargets
#[test]
fn momentum_settles_on_targets() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_bearing(Some(84.0)));
    let rate = friction_rate(milliseconds(140));
    // Moves (v - 1) / k = 4 degrees, then settles on the nearest of every 90.
    let momentum = CameraMomentum {
        bearing_velocity: Some(1.0 + 4.0 * rate),
        bearing_snap: Some(MomentumSnap { spacing: 90.0, tolerance: 7.0, duration: milliseconds(200), offset: 0.0 }),
        ..header_momentum()
    };
    let mut animation = AnimationOptions::default();
    let finished = counting_end(&mut animation);
    transform.start_momentum(&momentum, &animation);
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + seconds(1));
    assert_near(bearing_degrees(&transform), 88.0, 1e-9);
    assert_eq!(finished.get(), 0);
    assert!(transform.is_rotating());
    transform.update_transitions(start + milliseconds(1200));
    assert_near(bearing_degrees(&transform), 90.0, 1e-9);
    assert_eq!(finished.get(), 1);

    // Without motion, instant snaps settle at once: the bearing around the
    // circle, and the zoom on the nearest whole level. A value outside the
    // tolerance stays.
    let snap_only = CameraMomentum {
        bearing_snap: Some(MomentumSnap { spacing: 360.0, tolerance: 7.0, ..MomentumSnap::default() }),
        zoom_snap: Some(MomentumSnap { spacing: 1.0, tolerance: 0.5, ..MomentumSnap::default() }),
        pitch_snap: Some(MomentumSnap { spacing: 90.0, tolerance: 7.0, ..MomentumSnap::default() }),
        ..header_momentum()
    };
    transform.jump_to(&cam().with_bearing(Some(355.0)).with_zoom(Some(7.3)).with_pitch(Some(30.0)));
    transform.start_momentum(&snap_only, &animation);
    assert_eq!(finished.get(), 2);
    assert_near(bearing_degrees(&transform), 0.0, 1e-9);
    assert_near(transform.get_zoom(), 7.0, 1e-9);
    assert_near(pitch_degrees(&transform), 30.0, 1e-9);
    assert!(!transform.in_transition());

    // A spacing that doesn't divide the circle still finds the nearest
    // target around it: 270 is 1 degree from -89.
    let uneven = CameraMomentum {
        bearing_snap: Some(MomentumSnap { spacing: 270.0, tolerance: 7.0, ..MomentumSnap::default() }),
        ..header_momentum()
    };
    transform.jump_to(&cam().with_bearing(Some(-89.0)));
    transform.start_momentum(&uneven, &animation);
    assert_near(bearing_degrees(&transform), -90.0, 1e-9);
}

// gtest: Transform.MomentumStopsAPartThatLimitsHold
#[test]
fn momentum_stops_a_part_that_limits_hold() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.set_max_zoom(10.0);
    transform.jump_to(&cam().with_zoom(Some(10.0)).with_pitch(Some(0.0)));
    let momentum = CameraMomentum {
        zoom_velocity: Some(1.0),
        zoom_decay: MomentumDecay { stop_speed: 0.01, ..MomentumDecay::default() },
        pitch_velocity: Some(-20.0),
        ..header_momentum()
    };
    let (animation, ended) = record_end();
    transform.start_momentum(&momentum, &animation);
    transform.update_transitions(transform.get_latest_transition_start() + milliseconds(20));
    assert!(!transform.in_transition());
    assert_eq!(ended.get().expect("the momentum ended").velocity.zoom, 0.0);
    assert_near(transform.get_zoom(), 10.0, 1e-9);
}

// From Orbis's orb-camera (momentum/tests.rs,
// slow_releases_and_bad_decays_do_not_move): the release limits and the
// decays that can't glide, which the gtests don't cover.
#[test]
fn momentum_ignores_slow_releases_and_bad_decays() {
    let pan = MomentumDecay::PAN;
    assert!(Glide::plan(150.0, &pan).is_none());
    assert!(Glide::plan(f64::NAN, &pan).is_none());
    assert!(Glide::plan(f64::INFINITY, &MomentumDecay::FRICTION).is_none());
    assert!(Glide::plan(100.0, &friction(Duration::zero(), 1.0)).is_none());
    assert!(Glide::plan(100.0, &friction(milliseconds(100), 0.0)).is_none());
    // A release faster than the cap glides as the cap does.
    let capped = Glide::plan(50_000.0, &pan).unwrap();
    let max = Glide::plan(10_000.0, &pan).unwrap();
    assert_eq!(capped.distance, max.distance);
    assert_eq!(capped.duration, max.duration);
    assert_near(max.distance, 9990.0 / friction_rate(milliseconds(140)), 1e-9);
}

#[test]
fn momentum_parts_default_to_maplibre_native_ffis_defaults() {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(8.0)));
    let default = CameraMomentum::default();
    assert_eq!(default.pan_decay, MomentumDecay::PAN);
    assert_eq!(default.zoom_decay, MomentumDecay::ZOOM);
    assert_eq!(default.bearing_decay, MomentumDecay::BEARING);
    assert_eq!(default.pitch_decay, MomentumDecay::PITCH);

    // Slower than the pan's 200 px/s minimum: no glide.
    transform.start_momentum(
        &CameraMomentum { pan_velocity: Some(sc(150.0, 0.0)), ..CameraMomentum::default() },
        &AnimationOptions::default(),
    );
    assert!(!transform.in_transition());

    // Faster than its 10 000 px/s cap glides as the cap does.
    let glide = |speed: f64| {
        let mut transform = new_transform();
        transform.resize(Size::new(1000, 1000)).unwrap();
        transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(8.0)));
        let start = transform.get_lat_lng(WrapMode::Unwrapped);
        transform.start_momentum(
            &CameraMomentum { pan_velocity: Some(sc(speed, 0.0)), ..CameraMomentum::default() },
            &AnimationOptions::default(),
        );
        transform.update_transitions(TimePoint::max());
        transform.get_lat_lng(WrapMode::Unwrapped).longitude() - start.longitude()
    };
    assert_near(glide(20_000.0), glide(10_000.0), 1e-9);
    assert!(glide(10_000.0).abs() > glide(5_000.0).abs());
}

/// A transform 1000 px square at zoom 8, for the outcome tests.
fn outcome_transform() -> Transform {
    let mut transform = new_transform();
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(8.0)));
    transform
}

fn outcome(ended: &Rc<Cell<Option<TransitionEnd>>>) -> Option<TransitionOutcome> {
    ended.get().map(|end| end.outcome)
}

#[test]
fn an_ease_that_runs_to_its_end_completes() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    transform.ease_to(&cam().with_zoom(Some(10.0)), &AnimationOptions { duration: Some(seconds(1)), ..animation });
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + milliseconds(500));
    assert_eq!(outcome(&ended), None);
    transform.update_transitions(start + seconds(1));
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
    assert_eq!(ended.get().unwrap().velocity, CameraVelocity::default());
}

#[test]
fn cancelling_reports_cancelled_and_by_id_only_the_matching_commands() {
    let mut transform = outcome_transform();
    let (zoom, zoom_ended) = record_end();
    let (bearing, bearing_ended) = record_end();
    transform.ease_to(
        &cam().with_zoom(Some(10.0)),
        &AnimationOptions { duration: Some(seconds(1)), transition_id: Some(1), ..zoom },
    );
    transform.ease_to(&cam().with_bearing(Some(90.0)), &AnimationOptions { duration: Some(seconds(1)), ..bearing });
    assert!(transform.cancel_transitions_with_id(1));
    assert_eq!(outcome(&zoom_ended), Some(TransitionOutcome::Cancelled));
    assert_eq!(outcome(&bearing_ended), None);
    transform.cancel_transitions();
    assert_eq!(outcome(&bearing_ended), Some(TransitionOutcome::Cancelled));
}

#[test]
fn a_command_whose_fields_a_later_one_takes_is_replaced() {
    let mut transform = outcome_transform();
    let (first, first_ended) = record_end();
    transform.ease_to(&cam().with_zoom(Some(10.0)), &AnimationOptions { duration: Some(seconds(1)), ..first });
    transform.ease_to(&cam().with_zoom(Some(6.0)), &AnimationOptions::new(seconds(1)));
    assert_eq!(outcome(&first_ended), Some(TransitionOutcome::Replaced));
}

#[test]
fn a_command_that_keeps_some_fields_completes_or_is_cancelled() {
    // Losing the zoom leaves the bearing animating to its end: completed.
    let mut transform = outcome_transform();
    let (both, ended) = record_end();
    transform.ease_to(
        &cam().with_zoom(Some(10.0)).with_bearing(Some(90.0)),
        &AnimationOptions { duration: Some(seconds(1)), ..both.clone() },
    );
    let start = transform.get_latest_transition_start();
    transform.jump_to(&cam().with_zoom(Some(6.0)));
    assert_eq!(outcome(&ended), None);
    transform.update_transitions(start + seconds(1));
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));

    // Losing the zoom, then a cancellation of the bearing: cancelled.
    let mut transform = outcome_transform();
    let (both, ended) = record_end();
    transform.ease_to(
        &cam().with_zoom(Some(10.0)).with_bearing(Some(90.0)),
        &AnimationOptions { duration: Some(seconds(1)), ..both },
    );
    transform.jump_to(&cam().with_zoom(Some(6.0)));
    transform.cancel_transitions();
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Cancelled));
}

#[test]
fn a_command_that_fails_before_it_moves_is_cancelled() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    transform.ease_to(&cam().with_zoom(Some(f64::NAN)), &animation);
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Cancelled));
    assert_near(transform.get_zoom(), 8.0, 1e-9);
}

#[test]
fn dropping_the_transform_closes_its_commands() {
    let mut transform = outcome_transform();
    let (animation, ended) = record_end();
    transform.ease_to(&cam().with_zoom(Some(10.0)), &AnimationOptions { duration: Some(seconds(1)), ..animation });
    drop(transform);
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Closed));
}

/// Records each will-change ('w') and did-change ('d') with its change.
#[derive(Default)]
struct ChangeObserver {
    changes: RefCell<Vec<(char, CameraChange)>>,
}

impl ChangeObserver {
    fn take(&self) -> Vec<(char, CameraChange)> {
        std::mem::take(&mut *self.changes.borrow_mut())
    }
}

impl TransformObserver for ChangeObserver {
    fn on_camera_will_change(&self, change: CameraChange) {
        self.changes.borrow_mut().push(('w', change));
    }
    fn on_camera_did_change(&self, change: CameraChange) {
        self.changes.borrow_mut().push(('d', change));
    }
}

fn change(mode: CameraChangeMode, source: CameraChangeSource) -> CameraChange {
    CameraChange { mode, source }
}

#[test]
fn camera_changes_report_whether_a_command_or_a_momentum_made_them() {
    use CameraChangeMode::{Animated, Immediate};
    use CameraChangeSource::{Command, Momentum};
    let observer = Rc::new(ChangeObserver::default());
    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(8.0)));
    observer.take();

    // A jump starts and ends at once.
    transform.jump_to(&cam().with_zoom(Some(9.0)));
    assert_eq!(observer.take(), [('w', change(Immediate, Command)), ('d', change(Immediate, Command))]);

    // An ease and a momentum run together; each change that ends one
    // reports that one's source.
    transform.ease_to(&cam().with_zoom(Some(10.0)), &AnimationOptions::new(seconds(1)));
    let ease_start = transform.get_latest_transition_start();
    transform.start_momentum(
        // A 1.4 s glide, which outlasts the ease.
        &CameraMomentum { pan_velocity: Some(sc(10_000.0, 0.0)), ..CameraMomentum::default() },
        &AnimationOptions::default(),
    );
    assert_eq!(observer.take(), [('w', change(Animated, Command)), ('w', change(Animated, Momentum))]);
    transform.update_transitions(ease_start + seconds(1));
    assert_eq!(observer.take(), [('d', change(Animated, Command))]);
    transform.update_transitions(TimePoint::max());
    assert_eq!(observer.take(), [('d', change(Animated, Momentum))]);
}

fn delta(phase: GesturePhase) -> CameraDelta {
    CameraDelta { phase, ..CameraDelta::default() }
}

#[test]
fn a_gesture_reports_gesture_then_momentum_and_lasts_until_its_glide_ends() {
    use CameraChangeMode::{Animated, Immediate};
    use CameraChangeSource::{Gesture, Momentum};
    let observer = Rc::new(ChangeObserver::default());
    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(1000, 1000)).unwrap();
    transform.jump_to(&cam().with_center(Some(ll(0.0, 0.0))).with_zoom(Some(8.0)));
    observer.take();

    let pan = |phase| CameraDelta { offset: Some(sc(10.0, 0.0)), ..delta(phase) };
    transform.apply_camera_delta(&pan(GesturePhase::Begin)).unwrap();
    assert!(transform.is_gesture_in_progress());
    transform.apply_camera_delta(&pan(GesturePhase::Update)).unwrap();
    assert_eq!(
        observer.take(),
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
            momentum: Some(CameraMomentum { pan_velocity: Some(sc(2000.0, 0.0)), ..CameraMomentum::default() }),
            animation,
            ..delta(GesturePhase::End)
        })
        .unwrap();
    assert_eq!(observer.take(), [('w', change(Animated, Momentum))]);
    // The gesture's input ended, but it lasts until its momentum does.
    assert!(transform.is_gesture_in_progress());
    transform.update_transitions(TimePoint::max());
    assert_eq!(observer.take(), [('d', change(Animated, Momentum))]);
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
    assert!(!transform.is_gesture_in_progress());
}

#[test]
fn a_new_gesture_catches_the_last_ones_glide_and_keeps_the_gesture_going() {
    let mut transform = outcome_transform();
    transform.apply_camera_delta(&CameraDelta { offset: Some(sc(10.0, 0.0)), ..delta(GesturePhase::Begin) }).unwrap();
    let (animation, ended) = record_end();
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(CameraMomentum { pan_velocity: Some(sc(2000.0, 0.0)), ..CameraMomentum::default() }),
            animation,
            ..delta(GesturePhase::End)
        })
        .unwrap();
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + milliseconds(100));

    // A press mid-glide: the glide is cancelled, with its speed, and the new
    // gesture is in progress.
    transform.apply_camera_delta(&CameraDelta { offset: Some(sc(1.0, 0.0)), ..delta(GesturePhase::Begin) }).unwrap();
    let end = ended.get().expect("the glide ended");
    assert_eq!(end.outcome, TransitionOutcome::Cancelled);
    assert!(end.velocity.pan.x > 500.0 && end.velocity.pan.x < 2000.0, "{:?}", end.velocity);
    assert!(transform.is_gesture_in_progress());
    assert!(!transform.in_transition());

    // An earlier gesture's momentum can't end a later gesture: start a glide
    // for the new one, then begin a third gesture with Update, which keeps
    // the glide; the glide's end leaves the third gesture in progress.
    transform
        .apply_camera_delta(&CameraDelta {
            momentum: Some(CameraMomentum { pan_velocity: Some(sc(2000.0, 0.0)), ..CameraMomentum::default() }),
            ..delta(GesturePhase::End)
        })
        .unwrap();
    transform.apply_camera_delta(&CameraDelta { scale: Some(1.01), ..delta(GesturePhase::Update) }).unwrap();
    assert!(transform.in_transition());
    transform.update_transitions(TimePoint::max());
    assert!(!transform.in_transition());
    assert!(transform.is_gesture_in_progress());
    transform.apply_camera_delta(&delta(GesturePhase::End)).unwrap();
    assert!(!transform.is_gesture_in_progress());
}

#[test]
fn only_the_parts_a_gesture_changed_settle() {
    let snaps = CameraMomentum {
        zoom_snap: Some(MomentumSnap::ZOOM),
        bearing_snap: Some(MomentumSnap { duration: Duration::zero(), ..MomentumSnap::BEARING }),
        pitch_snap: Some(MomentumSnap::PITCH),
        ..CameraMomentum::default()
    };
    let settled = |gesture: CameraDelta| {
        let mut transform = outcome_transform();
        transform.jump_to(&cam().with_zoom(Some(8.3)).with_bearing(Some(3.0)).with_pitch(Some(4.0)));
        transform.apply_camera_delta(&CameraDelta { phase: GesturePhase::Begin, ..gesture }).unwrap();
        transform.apply_camera_delta(&CameraDelta { momentum: Some(snaps), ..delta(GesturePhase::End) }).unwrap();
        transform.update_transitions(TimePoint::max());
        (transform.get_zoom(), bearing_degrees(&transform), pitch_degrees(&transform))
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
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
    assert_near(transform.get_zoom(), 9.0, 1e-9);
    assert_near(bearing_degrees(&transform), 90.0, 1e-9);
    let under = transform.lat_lng_to_screen_coordinate(&ground);
    assert_near(under.x, anchor.x, 1e-6);
    assert_near(under.y, anchor.y, 1e-6);

    // Animated, both commands end the delta's one end function together.
    let (animation, ended) = record_end();
    transform
        .apply_camera_delta(&CameraDelta {
            offset: Some(sc(10.0, 0.0)),
            scale: Some(2.0),
            animation: AnimationOptions { duration: Some(seconds(1)), ..animation },
            ..CameraDelta::default()
        })
        .unwrap();
    let start = transform.get_latest_transition_start();
    transform.update_transitions(start + milliseconds(500));
    assert_eq!(outcome(&ended), None);
    transform.update_transitions(start + seconds(1));
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
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
// doesn't; on a flat map the anchor changes nothing.
#[test]
fn a_drag_delta_holds_its_anchor() {
    let offset = sc(30.0, 12.0);
    let cursor = sc(330.0, 200.0);
    let drag = |transform: &mut Transform, anchor: Option<ScreenCoordinate>| {
        transform.apply_camera_delta(&CameraDelta { offset: Some(offset), anchor, ..CameraDelta::default() }).unwrap();
        transform.get_lat_lng(WrapMode::Unwrapped)
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
    assert_eq!(through_the_delta, direct.get_lat_lng(WrapMode::Unwrapped));
    assert_eq!(held.get_zoom(), direct.get_zoom());
    let around_the_center = drag(&mut globe(), None);
    assert!(
        (through_the_delta.latitude() - around_the_center.latitude()).abs() > 0.1
            || (through_the_delta.longitude() - around_the_center.longitude()).abs() > 0.1,
        "{through_the_delta:?} {around_the_center:?}"
    );

    let flat = || {
        let mut transform = new_transform();
        transform.resize(Size::new(512, 512)).unwrap();
        transform.jump_to(&cam().with_center(Some(ll(30.0, 20.0))).with_zoom(Some(3.0)));
        transform
    };
    assert_eq!(drag(&mut flat(), Some(cursor)), drag(&mut flat(), None));
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
            animation: AnimationOptions::new(seconds(1)),
            ..delta(GesturePhase::End)
        }),
        Err(CameraError::MomentumWithAnimation)
    );
    assert_eq!(
        check(CameraDelta {
            offset: Some(sc(1.0, 0.0)),
            scale: Some(2.0),
            anchor: Some(sc(1.0, 1.0)),
            animation: AnimationOptions::new(seconds(1)),
            ..CameraDelta::default()
        }),
        Err(CameraError::AnimatedAnchoredOffset)
    );
    // A rejected delta changes nothing; an empty one ends at once.
    assert_near(transform.get_zoom(), 8.0, 1e-9);
    let (animation, ended) = record_end();
    transform.apply_camera_delta(&CameraDelta { animation, ..CameraDelta::default() }).unwrap();
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Completed));
}

#[test]
fn an_update_with_a_phase_reports_a_gesture_and_begin_cancels_animations() {
    use CameraChangeSource::Gesture;
    let observer = Rc::new(ChangeObserver::default());
    let mut transform = Transform::new(observer.clone(), ConstrainMode::HeightOnly, ViewportMode::Default);
    transform.resize(Size::new(1000, 1000)).unwrap();
    let (animation, ended) = record_end();
    transform.ease_to(&cam().with_zoom(Some(6.0)), &AnimationOptions { duration: Some(seconds(1)), ..animation });

    // Update keeps the running animation.
    transform.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: cam().with_bearing(Some(30.0)),
        animation: AnimationOptions::new(milliseconds(300)),
        phase: GesturePhase::End,
    });
    assert_eq!(outcome(&ended), None);
    assert!(!transform.is_gesture_in_progress());
    assert_eq!(observer.take().last().map(|(_, c)| c.source), Some(Gesture));

    // Begin cancels it.
    transform.update_camera(&CameraUpdate {
        camera: cam().with_pitch(Some(10.0)),
        phase: GesturePhase::Begin,
        ..CameraUpdate::default()
    });
    assert_eq!(outcome(&ended), Some(TransitionOutcome::Cancelled));
    assert!(transform.is_gesture_in_progress());
    assert_near(pitch_degrees(&transform), 10.0, 1e-9);
}

#[test]
fn with_a_frame_time_commands_start_at_the_latest_frame() {
    let mut transform = outcome_transform();
    let frame = Clock::now();
    transform.set_frame_time(Some(frame));
    // Made between frames, the ease starts at the latest frame's time, so
    // the next frame shows it one frame in.
    let linear = UnitBezier::new(0.0, 0.0, 1.0, 1.0);
    transform.ease_to(
        &cam().with_zoom(Some(9.0)),
        &AnimationOptions { duration: Some(milliseconds(160)), easing: Some(linear), ..AnimationOptions::default() },
    );
    assert_eq!(transform.get_latest_transition_start(), frame);
    transform.update_transitions(frame + milliseconds(16));
    assert_near(transform.get_zoom(), 8.1, 1e-4);

    // A momentum starts there too.
    transform.start_momentum(
        &CameraMomentum { pan_velocity: Some(sc(1000.0, 0.0)), ..CameraMomentum::default() },
        &AnimationOptions::default(),
    );
    assert_eq!(transform.get_latest_transition_start(), frame);

    // Without one, commands start at Clock::now().
    transform.set_frame_time(None);
    let before = Clock::now();
    transform.ease_to(&cam().with_bearing(Some(10.0)), &AnimationOptions::new(seconds(1)));
    assert!(transform.get_latest_transition_start() >= before);
}

#[test]
fn a_stale_frame_time_doesnt_start_commands_in_the_past() {
    let mut transform = outcome_transform();
    // The host set a frame time a second ago and has drawn no frame since.
    transform.set_frame_time(Some(Clock::now() - seconds(1)));
    assert_eq!(transform.frame_time(), None);
    let before = Clock::now();
    transform.ease_to(&cam().with_zoom(Some(9.0)), &AnimationOptions::new(milliseconds(300)));
    let start = transform.get_latest_transition_start();
    assert!(start >= before, "{start:?} before {before:?}");
    // So the ease hasn't run its course by the next frame.
    transform.update_transitions(start + milliseconds(16));
    assert!(transform.in_transition());
    assert!(transform.get_zoom() < 9.0);

    // A fresh one is used, from a future frame too.
    let fresh = Clock::now() + milliseconds(5);
    transform.set_frame_time(Some(fresh));
    assert_eq!(transform.frame_time(), Some(fresh));
}
