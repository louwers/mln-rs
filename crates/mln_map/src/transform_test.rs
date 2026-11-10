// Ports test/map/transform.test.cpp (TEST(Transform, *) and TEST(Camera, *)).
//
// The C++ AnimationOptions callbacks capture the Transform by reference and
// assert on it. The Rust callbacks can't borrow the Transform that is calling
// them (`ease_to`/`update_transitions` hold it mutably), so they record that
// they ran (`CallbackLog`) and the test asserts on the Transform right after
// the call that ran them. Nothing changes the state between a callback and
// the return of that call: after `transition_frame_fn` the frame closure only
// calls `on_camera_is_changing`, after `transition_finish_fn` the finish
// closure only calls `on_camera_did_change`, and `update_transitions` only
// moves the closures back.

use std::cell::{Cell, RefCell};

use mln_map_types::util_camera::Camera as UtilCamera;
use mln_util::UnitBezier;
use mln_util::chrono::{milliseconds, seconds};
use mln_util::constants::{LATITUDE_MAX, LONGITUDE_MAX, MAX_ZOOM};
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
        options.transition_finish_fn = Some(Rc::new(move || finishes.set(finishes.get() + 1)));
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
        fn on_camera_will_change(&self, _mode: CameraChangeMode) {
            self.camera_will_change_count.set(self.camera_will_change_count.get() + 1);
        }

        fn on_camera_did_change(&self, _mode: CameraChangeMode) {
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

// The tests below port maplibre-native-ffi's patches 0015 (independent
// camera animations) and 0043 (cancel transitions by id).

const LINEAR: UnitBezier = UnitBezier::new(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0);

/// A finish callback that counts its calls.
fn counting_finish(options: &mut AnimationOptions) -> Rc<Cell<u32>> {
    let count = Rc::new(Cell::new(0));
    let finished = count.clone();
    options.transition_finish_fn = Some(Rc::new(move || finished.set(finished.get() + 1)));
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
    let zoom_finished = counting_finish(&mut zoom);
    transform.ease_to(&cam().with_zoom(Some(12.0)), &zoom);
    let zoom_start = transform.get_latest_transition_start();
    transform.update_transitions(zoom_start + milliseconds(500));
    assert_near(transform.get_zoom(), 9.0, 1e-5);

    let mut center = AnimationOptions { easing: Some(LINEAR), ..AnimationOptions::new(seconds(8)) };
    let center_finished = counting_finish(&mut center);
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
    let finished = counting_finish(&mut animation);
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
    let flight_finished = counting_finish(&mut flight);
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
    let anchor_finished = counting_finish(&mut anchor_animation);
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
    fn on_camera_did_change(&self, _mode: CameraChangeMode) {
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
        Some(Rc::new(move || observer.events.borrow_mut().push(event)) as Rc<dyn Fn()>)
    };
    let position = AnimationOptions { transition_finish_fn: push(1), ..AnimationOptions::new(seconds(5)) };
    let bearing = AnimationOptions { transition_finish_fn: push(2), ..AnimationOptions::new(seconds(10)) };
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
    let zoom_finished = counting_finish(&mut zoom);
    let mut bearing = AnimationOptions { transition_id: Some(2), ..AnimationOptions::new(seconds(4)) };
    let bearing_finished = counting_finish(&mut bearing);
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
