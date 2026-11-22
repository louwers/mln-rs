use glam::{DQuat, DVec3, DVec4};
use mln_style_types::ProjectionDefinition;
use mln_util::constants::{DEFAULT_FOV, LATITUDE_MAX};
use mln_util::geo::LatLng;
use mln_util::projection::Projection;
use mln_util::size::Size;

use super::TransformState;
use crate::{Camera, CameraLimits, LogicalExtent, MapView, ProjectionMode, Viewport};

/// A 512 by 512 view at `pitch` degrees, looking north.
fn pitched(pitch: f64) -> TransformState {
    MapView {
        camera: Camera {
            center: LatLng::new(0.0, -60.0).unwrap(),
            zoom: 5.0,
            pitch,
            fov: mln_math::rad2deg(DEFAULT_FOV),
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state()
    .unwrap()
}

/// The ground north of the center through `state`'s projection matrix,
/// every hundredth of a degree up to 85 degrees north: each point's row
/// from the top of the view, and its clip z over w.
fn meridian(state: &TransformState) -> Vec<(f64, f64)> {
    let proj_matrix = state.view().projection_matrix_with(1.0, false);
    let height = f64::from(state.view().size().height);
    (0..8500)
        .map(|step| {
            let lat_lng = LatLng::new(f64::from(step) / 100.0, -60.0).unwrap();
            let point = Projection::project(&lat_lng, state.view().scale());
            let clip = proj_matrix * DVec4::new(point.x, point.y, 0.0, 1.0);
            ((1.0 - clip.y / clip.w) / 2.0 * height, clip.z / clip.w)
        })
        .collect()
}

/// At high pitch the far plane ends the map at GL JS's horizon, below
/// the top of the view: the ground drawn below the horizon is nearer
/// than the far plane, and the ground drawn above it, up to the flat
/// map's geometric horizon, is past it.
#[test]
fn the_map_ends_at_the_horizon() {
    let state = pitched(85.0);
    let horizon_row = 256.0 - state.view().mercator_horizon();
    let ground = meridian(&state);
    assert!(ground.iter().filter(|(row, _)| *row > horizon_row).all(|(_, depth)| *depth < 1.0));
    let past: Vec<_> = ground.iter().filter(|(row, _)| *row < horizon_row - 1.0).collect();
    assert_ne!(past.len(), 0);
    assert!(past.iter().all(|(row, depth)| *row > 0.0 && *depth > 1.0));
}

/// Below about 68 degrees of pitch the horizon is above the top of the
/// view, and the far plane reaches the ground at the top.
#[test]
fn the_map_fills_the_view_at_lower_pitch() {
    let state = pitched(60.0);
    assert!(state.view().mercator_horizon() > 256.0);
    let ground = meridian(&state);
    assert!(ground.iter().filter(|(row, _)| *row >= 0.0).all(|(_, depth)| *depth < 1.0));
    assert!(ground.iter().any(|(row, _)| *row < 0.0));
}

/// The globe's `asinh(tan(φ))` and the Mercator branch's
/// `0.5 * ln((1 + sin φ) / (1 - sin φ))` give the same y inside the
/// Mercator range, so the center doesn't jump when the projection
/// changes.
#[test]
fn globe_and_mercator_latitude_to_y_agree() {
    for latitude in [-LATITUDE_MAX, -60.0, -1e-9, 0.0, 30.0, 80.0, LATITUDE_MAX] {
        let phi = mln_math::deg2rad(latitude);
        let globe = phi.tan().asinh();
        let f = phi.sin();
        let mercator = 0.5 * ((1.0 + f) / (1.0 - f)).ln();
        assert!((globe - mercator).abs() <= 1e-12, "{latitude}: {globe} vs {mercator}");
    }
}

#[test]
fn a_center_too_far_to_place_leaves_the_camera_unchanged() {
    use mln_util::geo::WrapMode;

    let mut state = pitched(30.0);
    let center = state.view().center(WrapMode::Unwrapped);
    let zoom = state.view().zoom();
    let projection = state.view().projection_matrix();
    state.set_lat_lng_zoom(&LatLng::with_mode(0.0, f64::MAX, WrapMode::Unwrapped).unwrap(), 22.0);
    assert_eq!(center, state.view().center(WrapMode::Unwrapped));
    assert_eq!(zoom, state.view().zoom());
    assert_eq!(projection, state.view().projection_matrix());
}

/// Every setter that takes a number, given NaN, infinities and extreme
/// finite values, on a flat map and a globe: the state's reads that
/// return locations and matrices still don't panic.
#[test]
fn numbers_out_of_range_do_not_panic() {
    use mln_style_types::ProjectionType;
    use mln_util::geo::{EdgeInsets, ScreenCoordinate, WrapMode};

    use crate::FreeCameraOptions;

    type Setter = (&'static str, fn(&mut TransformState, f64));
    let values =
        [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -f64::MAX, f64::MIN_POSITIVE, 0.0, 1e300, -1e300];
    let setters: &[Setter] = &[
        ("constrained scale", |s, v| s.constrain_position(v)),
        ("bearing", |s, v| s.set_angle(v)),
        ("field of view", |s, v| s.set_fov(v)),
        ("roll", |s, v| s.set_roll(v)),
        ("pitch", |s, v| s.set_pitch(v)),
        ("x skew", |s, v| {
            s.set_axonometric(true);
            s.set_x_skew(v);
        }),
        ("y skew", |s, v| {
            s.set_axonometric(true);
            s.set_y_skew(v);
        }),
        ("min zoom", |s, v| {
            let _ = s.set_limits(CameraLimits { min_zoom: v, ..*s.limits() });
        }),
        ("max zoom", |s, v| {
            let _ = s.set_limits(CameraLimits { max_zoom: v, ..*s.limits() });
        }),
        ("min pitch", |s, v| {
            let _ = s.set_limits(CameraLimits { min_pitch: v, ..*s.limits() });
        }),
        ("max pitch", |s, v| {
            let _ = s.set_limits(CameraLimits { max_pitch: v, ..*s.limits() });
        }),
        ("center altitude", |s, v| s.set_center_altitude(v)),
        ("terrain min elevation", |s, v| s.set_terrain_min_elevation(Some(v))),
        ("zoom", |s, v| s.set_lat_lng_zoom(&LatLng::new(10.0, 10.0).unwrap(), v)),
        ("longitude", |s, v| {
            if let Ok(lat_lng) = LatLng::with_mode(0.0, v, WrapMode::Unwrapped) {
                s.set_lat_lng_zoom(&lat_lng, 22.0);
            }
        }),
        ("free camera position", |s, v| {
            s.set_free_camera_options(&FreeCameraOptions { position: Some(DVec3::splat(v)), orientation: None });
        }),
        ("free camera altitude", |s, v| {
            s.set_free_camera_options(&FreeCameraOptions {
                position: Some(DVec3::new(0.5, 0.5, v)),
                orientation: None,
            });
        }),
        ("free camera orientation", |s, v| {
            s.set_free_camera_options(&FreeCameraOptions {
                position: None,
                orientation: Some(DQuat::from_xyzw(v, 0.0, 0.0, 1.0)),
            });
        }),
        ("edge insets", |s, v| s.set_padding(&EdgeInsets { top: v, left: v, bottom: v, right: v })),
        ("frustum offset", |s, v| {
            s.set_frustum_offset(&EdgeInsets { top: v, left: v, bottom: v, right: v });
        }),
        ("move anchor", |s, v| {
            let _ = s.move_lat_lng(&LatLng::new(1.0, 1.0).unwrap(), &ScreenCoordinate::new(v, v));
        }),
    ];
    for globe in [false, true] {
        for (name, set) in setters {
            for &value in &values {
                let mut state = pitched(30.0);
                if globe {
                    state.set_projection_definition(&ProjectionDefinition::new(ProjectionType::Globe));
                }
                let read = std::panic::catch_unwind(move || {
                    set(&mut state, value);
                    let view = state.view();
                    let center = view.center(WrapMode::Unwrapped);
                    let _ = view.lat_lng_near_center(&LatLng::new(1.0, 1.0).unwrap());
                    let _ = state.constrained_center(&center, view.zoom());
                    let _ = view.center_altitude();
                    let _ = view.projection_matrix();
                    let _ = view.free_camera_options();
                    let _ = view.lat_lng_to_screen_coordinate(&center);
                    let _ =
                        view.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(256.0, 256.0), WrapMode::Unwrapped);
                });
                assert!(read.is_ok(), "globe: {globe}, {name}: {value:e}");
            }
        }
    }
}

#[test]
fn limits_apply_as_a_whole() {
    let mut state = pitched(30.0);
    // A minimum above the current maximum takes a maximum above it at once.
    let raised = CameraLimits { min_zoom: 23.0, max_zoom: 24.0, ..*state.limits() };
    state.set_limits(raised).unwrap();
    assert_eq!((23.0, 24.0), (state.limits().min_zoom, state.limits().max_zoom));
    // Limits that leave the camera nowhere to go change nothing.
    let crossed = CameraLimits { min_zoom: 10.0, max_zoom: 5.0, ..*state.limits() };
    assert!(state.set_limits(crossed).is_err());
    // Even when clamping would make them meet.
    let crossed_past_the_range = CameraLimits { min_zoom: 27.0, max_zoom: 26.0, ..*state.limits() };
    assert!(state.set_limits(crossed_past_the_range).is_err());
    assert_eq!(raised, *state.limits());
    // Out of the range any map allows, they are clamped to it.
    let wide = CameraLimits { min_zoom: -3.0, max_pitch: 4.0, ..*state.limits() };
    state.set_limits(wide).unwrap();
    assert_eq!(0.0, state.limits().min_zoom);
    assert_eq!(mln_util::constants::PITCH_MAX, state.limits().max_pitch);
}

#[test]
fn a_maximum_zoom_below_the_least_zoom_the_view_allows_changes_nothing() {
    // The world fills a 768 pixel high view at zoom log2(768 / 512).
    let mut state = pitched(0.0);
    state.set_size(Size::new(512, 768));
    let limits = *state.limits();
    assert!(state.set_limits(CameraLimits { max_zoom: 0.0, ..limits }).is_err());
    assert_eq!(limits, *state.limits());
    let least_zoom = 1.5f64.log2();
    state.set_limits(CameraLimits { max_zoom: least_zoom, ..limits }).unwrap();
    assert_eq!(least_zoom, state.limits().max_zoom);
}

#[test]
fn equal_zoom_limits_just_below_the_least_zoom_the_view_allows_apply() {
    // A 1024 pixel high view allows zoom 1, the scale that a zoom this close
    // to 1 rounds to.
    let mut state = pitched(0.0);
    state.set_size(Size::new(512, 1024));
    let zoom = 0.999_999_999_9;
    state.set_limits(CameraLimits { min_zoom: zoom, max_zoom: zoom, ..*state.limits() }).unwrap();
}

/// A 301 by 200 view of the Alps, level or with pitch, roll, padding and a
/// center below sea level.
fn alpine(pitched: bool) -> TransformState {
    use mln_util::geo::EdgeInsets;

    let (center_altitude, pitch, roll, top) = if pitched { (-800.0, 72.5, 9.0, 60.0) } else { (0.0, 0.0, 0.0, 0.0) };
    MapView {
        camera: Camera {
            center: LatLng::new(46.5, 7.9).unwrap(),
            center_altitude,
            padding: EdgeInsets::new(top, 0.0, 10.0, 25.0),
            zoom: 12.25,
            bearing: 20.0,
            pitch,
            roll,
            fov: 37.0,
        },
        extent: LogicalExtent::new(Size::new(301, 200), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state()
    .unwrap()
}

/// Fails unless the numbers are the same but for the last bits, which differ
/// by the target's math library.
fn assert_same(expected: &[f64], actual: &[f64]) {
    for (expected, actual) in expected.iter().zip(actual) {
        let tolerance = 1e-12 * expected.abs().max(1.0);
        assert!((expected - actual).abs() <= tolerance, "{actual} isn't {expected}");
    }
}

/// Without terrain the planes and matrices are those of the flat map, and a
/// terrain that goes away restores them.
#[test]
fn matrices_without_terrain_are_unchanged() {
    // Each view's far plane, projection matrix, and the z row of its matrix
    // with the near plane at a tenth of the distance to the center.
    let expected = [
        (
            alpine(false),
            301.857_181_237_032_25,
            [
                1.866_076_615_892_337_9,
                1.022_190_496_282_749_3,
                0.0,
                0.0,
                0.679_196_343_045_016_2,
                -2.808_445_306_917_968_7,
                0.0,
                0.0,
                0.007_517_278_564_121_545,
                -0.004_525_401_695_601_170_5,
                -0.091_109_701_677_039_9,
                -0.090_508_033_912_023_4,
                -3_028_316.878_903_641,
                1_147_156.446_909_028_3,
                298.848_628_475_551_95,
                298.868_496_274_289_27,
            ],
            [0.0, 0.0, -0.109_746_911_712_850_36, 298.233_205_214_500_34],
        ),
        (
            alpine(true),
            54_505.332_684_338_09,
            [
                1.784_059_983_608_735_4,
                0.661_384_306_806_117_8,
                0.326_202_377_525_688_9,
                0.326_190_408_187_028_27,
                0.833_051_235_020_185_3,
                -0.450_162_381_905_947_7,
                -0.896_233_666_424_242_3,
                -0.896_200_780_936_546_3,
                -0.024_554_814_939_620_46,
                0.261_608_349_779_387_8,
                -0.027_217_289_382_642_18,
                -0.027_216_290_699_074_892,
                -3_057_312.575_182_401,
                -463_638.634_713_357_43,
                366_351.964_219_291,
                366_340.521_635_790_1,
            ],
            [0.326_537_697_417_830_06, -0.897_154_950_256_194, -0.027_245_267_408_459_74, 366_672.527_115_380_4],
        ),
    ];
    for (mut state, far_z, projection, clipped_z) in expected {
        let far_z: f64 = far_z;
        for _ in 0..2 {
            let view = state.view();
            assert_eq!(1.0, view.near_z());
            assert_same(&[far_z], &[view.far_z()]);
            assert_same(&projection, &view.projection_matrix().to_cols_array());
            let near_clipped = f64::from((0.1 * view.camera_to_center_distance()) as u16);
            let clipped = view.projection_matrix_with(near_clipped, false).row(2).to_array();
            assert_same(&clipped_z, &clipped);

            let before = view.clone();
            state.set_terrain_min_elevation(Some(-120.0));
            assert_ne!(before.far_z(), state.view().far_z());
            state.set_terrain_min_elevation(None);
            assert_eq!(before, *state.view());
        }
    }
}

/// Meters scale as at the center, as the center's altitude does, so a point
/// at the center's altitude draws at the center of the view, even where the
/// camera is far from the center, over another latitude (GL JS's
/// `pixelsPerMeter`).
#[test]
fn the_center_at_its_altitude_draws_at_the_center() {
    for (zoom, center_altitude) in [(0.0, 9000.0), (3.0, -4000.0), (14.0, 1200.0)] {
        let state = MapView {
            camera: Camera {
                center: LatLng::new(20.0, 10.0).unwrap(),
                center_altitude,
                zoom,
                pitch: 60.0,
                bearing: 45.0,
                fov: mln_math::rad2deg(DEFAULT_FOV),
                ..Camera::default()
            },
            extent: LogicalExtent::new(Size::new(512, 512), 1.0),
            projection_mode: ProjectionMode::default(),
            viewport: Viewport::default(),
            projection: ProjectionDefinition::default(),
        }
        .to_state()
        .unwrap();
        let view = state.view();
        let center = Projection::project(&view.center(mln_util::geo::WrapMode::Unwrapped), view.scale());
        let clip = view.projection_matrix() * DVec4::new(center.x, center.y, center_altitude, 1.0);
        assert!((clip.x / clip.w).abs() < 1e-9 && (clip.y / clip.w).abs() < 1e-9, "zoom {zoom}: {clip}");
    }
}

/// The center's altitude is kept in meters: changes of the zoom and the
/// center, including a move around an anchor, leave it as it was set, and
/// the matrices still draw the center at that altitude at the center of
/// the view.
#[test]
fn the_center_altitude_holds_in_meters_through_zoom_and_center_changes() {
    use mln_util::geo::{ScreenCoordinate, WrapMode};

    let mut state = pitched(60.0);
    state.set_center_altitude(1200.0);
    let draws_at_center = |state: &TransformState| {
        let view = state.view();
        let center = Projection::project(&view.center(WrapMode::Unwrapped), view.scale());
        let clip = view.projection_matrix() * DVec4::new(center.x, center.y, 1200.0, 1.0);
        assert!((clip.x / clip.w).abs() < 1e-9 && (clip.y / clip.w).abs() < 1e-9, "{clip}");
    };
    draws_at_center(&state);

    state.set_lat_lng_zoom(&LatLng::new(0.0, -60.0).unwrap(), 12.5);
    assert_eq!(state.view().center_altitude(), 1200.0);
    draws_at_center(&state);

    state.set_lat_lng_zoom(&LatLng::new(61.0, 25.0).unwrap(), 12.5);
    assert_eq!(state.view().center_altitude(), 1200.0);
    draws_at_center(&state);

    let lat_lng = state.view().screen_coordinate_to_lat_lng(&ScreenCoordinate::new(100.0, 150.0), WrapMode::Unwrapped);
    state.move_lat_lng(&lat_lng.unwrap(), &ScreenCoordinate::new(300.0, 350.0)).unwrap();
    assert_ne!(state.view().center(WrapMode::Unwrapped).latitude(), 61.0);
    assert_eq!(state.view().center_altitude(), 1200.0);
    draws_at_center(&state);

    state.set_lat_lng_zoom(&state.view().center(WrapMode::Unwrapped), 4.0);
    assert_eq!(state.view().center_altitude(), 1200.0);
    draws_at_center(&state);
}

/// Moving a location on a plane above sea level to a point puts it there,
/// at that height, with the zoom, the angles and the center's altitude
/// kept; at sea level it is `move_lat_lng`, to the bit.
#[test]
fn moving_a_location_on_a_plane_puts_it_at_the_point() {
    use mln_util::geo::{ScreenCoordinate, WrapMode};

    let mut state = pitched(55.0);
    state.set_lat_lng_zoom(&LatLng::new(30.0, 20.0).unwrap(), 12.0);
    state.set_center_altitude(400.0);
    let (from, to) = (ScreenCoordinate::new(140.0, 120.0), ScreenCoordinate::new(380.0, 200.0));
    for height in [0.0, 400.0, 1500.0] {
        let mut moved = state.clone();
        let location = moved.view().screen_coordinate_to_lat_lng_at(&from, height, WrapMode::Unwrapped).unwrap();
        moved.move_lat_lng_at(&location, &to, height).unwrap();
        // The center's altitude in meters draws a little higher or lower at
        // the center's new latitude.
        let landed = moved.view().lat_lng_to_screen_coordinate_at(&location, height);
        assert!((landed.x - to.x).hypot(landed.y - to.y) < 1e-2, "{height} m: {landed:?}");
        let (before, after) = (state.view(), moved.view());
        assert_eq!(
            (before.zoom(), before.angle(), before.pitch(), before.center_altitude()),
            (after.zoom(), after.angle(), after.pitch(), after.center_altitude())
        );
        if height == 0.0 {
            let mut flat = state.clone();
            flat.move_lat_lng(&location, &to).unwrap();
            assert_eq!(flat, moved);
        }
    }
}

/// The eye stands back from the center along the line of sight, and the
/// near plane's middle point is on that line, the near plane's distance
/// further. Raising the eye keeps the ground under it and the center it
/// looks at, with its altitude, the bearing and the roll.
#[test]
fn raising_the_eye_keeps_its_place_and_the_center() {
    use mln_util::geo::WrapMode;

    let mut state = pitched(70.0);
    state.set_limits(CameraLimits { max_pitch: mln_util::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    state.set_angle(0.5);
    state.set_roll(0.1);
    state.set_center_altitude(1000.0);
    state.set_terrain_min_elevation(Some(800.0));
    let view = state.view().clone();
    let meters_per_pixel = view.meters_per_pixel();
    let world_size = Projection::world_size(view.scale());
    let eye = view.eye_position();
    let rise = mln_math::deg2rad(70.0).cos() * view.camera_to_center_distance() * meters_per_pixel;
    assert!((eye.z - (1000.0 + rise)).abs() < 1e-6, "{eye}");
    let middle = view.near_plane_points()[4];
    let to_middle = DVec3::new(
        (middle.x - eye.x) * world_size,
        (middle.y - eye.y) * world_size,
        (middle.z - eye.z) / meters_per_pixel,
    );
    assert!((to_middle.length() - view.near_z()).abs() < 1e-6, "{to_middle}");
    assert!((to_middle.z / to_middle.length() + mln_math::deg2rad(70.0).cos()).abs() < 1e-9, "{to_middle}");

    assert!(state.raise_eye(500.0));
    let raised = state.view();
    let center = |view: &crate::View| view.center(WrapMode::Unwrapped);
    assert!((center(raised).latitude() - center(&view).latitude()).abs() < 1e-9);
    assert!((center(raised).longitude() - center(&view).longitude()).abs() < 1e-9);
    assert_eq!((1000.0, 0.5, 0.1), (raised.center_altitude(), raised.angle(), raised.roll()));
    assert!(raised.pitch() < view.pitch() && raised.zoom() < view.zoom());
    let raised_eye = raised.eye_position();
    assert!((raised_eye.truncate() - eye.truncate()).length() * world_size < 1e-6, "{raised_eye} {eye}");
    assert!((raised_eye.z - (eye.z + 500.0)).abs() < 1e-6, "{raised_eye} {eye}");
}

/// An eye below sea level raised to it looks level at a center at sea
/// level, from as far as it stood across the ground: GL JS's pitch 90 and
/// zoom for the camera of `high-pitch/terrain-pitch95`.
#[test]
fn an_eye_raised_level_with_the_center_looks_level() {
    let mut state = pitched(95.0);
    state.set_limits(CameraLimits { max_pitch: mln_util::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    state.set_pitch(mln_math::deg2rad(95.0));
    let eye = state.view().eye_position();
    assert!(eye.z < 0.0, "{eye}");
    assert!(state.raise_eye(-eye.z));
    assert!((state.view().pitch() - mln_math::deg2rad(90.0)).abs() < 1e-12);
    let zoom = 5.0 + (1.0 / mln_math::deg2rad(95.0).sin()).log2();
    assert!((state.view().zoom() - zoom).abs() < 1e-12, "{}", state.view().zoom());
}

/// A view at latitude 45, zoom 14 and pitch 60, turned, with the center at
/// sea level, and the point 1000 m up on its center's line of sight, where
/// ground that rises in front of the center would meet it.
fn looking_at_a_rise() -> (TransformState, crate::GroundHit) {
    use mln_util::geo::WrapMode;

    let mut state = pitched(60.0);
    state.set_angle(0.5);
    state.set_lat_lng_zoom(&LatLng::new(45.0, 10.0).unwrap(), 14.0);
    let view = state.view();
    let center = view.lat_lng_to_screen_coordinate(&view.center(WrapMode::Unwrapped));
    let (near, far) = view.line_of_sight(&center).unwrap();
    let point = near + (far - near) * ((1000.0 - near.z) / (far.z - near.z));
    (state, crate::GroundHit { world: point.truncate(), height: 1000.0 })
}

/// Meters along the ground at latitude 45 in a unit of Web Mercator at zoom 0.
fn meters_at_45() -> f64 {
    Projection::altitude_meters_per_pixel(45.0, 0.0) * mln_util::constants::TILE_SIZE_D
}

/// Moving the center along the view onto a point of its line of sight keeps
/// the eye, the pitch and the bearing, and zooms in to the nearer center,
/// which stands where the view at the new latitude reaches the point's
/// height, off it by far less than a meter.
#[test]
fn recentering_keeps_the_eye_and_moves_the_center_along_the_view() {
    use mln_util::geo::WrapMode;

    let (mut state, hit) = looking_at_a_rise();
    let before = state.view().clone();
    assert!(state.recenter_on(&hit));
    let view = state.view();
    assert_eq!((before.pitch(), before.angle()), (view.pitch(), view.angle()));
    let moved = view.eye_position() - before.eye_position();
    let moved = DVec3::new(moved.x * meters_at_45(), moved.y * meters_at_45(), moved.z);
    assert!(moved.length() < 1e-5, "the eye moved {moved} m");
    assert!(view.zoom() > before.zoom() + 0.1, "{}", view.zoom());
    let center = Projection::project(&view.center(WrapMode::Unwrapped), 1.0 / mln_util::constants::TILE_SIZE_D);
    let off = DVec3::new(
        (center.x - hit.world.x) * meters_at_45(),
        (center.y - hit.world.y) * meters_at_45(),
        view.center_altitude() - hit.height,
    );
    assert!(off.length() > 1e-3 && off.length() < 1.0, "the center stands {off} m off the point");
}

/// A point behind the eye, or a zoom past the limit, leaves the state as it
/// is.
#[test]
fn recentering_refuses_a_point_behind_the_eye_or_past_the_zoom_limit() {
    let (mut state, hit) = looking_at_a_rise();
    let before = state.clone();
    let eye = state.view().eye_position();
    let behind = crate::GroundHit { world: hit.world + (eye.truncate() - hit.world) * 2.0, height: hit.height };
    assert!(!state.recenter_on(&behind));
    assert_eq!(before, state);
    state.set_limits(CameraLimits { max_zoom: 14.0, ..*state.limits() }).unwrap();
    let before = state.clone();
    assert!(!state.recenter_on(&hit));
    assert_eq!(before, state);
}

/// With terrain, the near and far planes are GL JS's (`calculateNearFarZ`
/// in `mercator_camera_math.ts`): the camera below sea level is GL JS's own
/// test, with its expected far plane, and the others pin the same function
/// on other cameras.
#[test]
fn terrain_planes_are_gl_js_planes() {
    use mln_util::geo::EdgeInsets;

    struct Case {
        size: Size,
        center: (f64, f64),
        center_altitude: f64,
        zoom: f64,
        pitch: f64,
        top_padding: f64,
        min_elevation: f64,
        near_z: f64,
        far_z: f64,
    }
    let cases = [
        // Ground above sea level, the center on it.
        Case {
            size: Size::new(500, 400),
            center: (46.5, 7.9),
            center_altitude: 1200.0,
            zoom: 12.25,
            pitch: 60.0,
            top_padding: 0.0,
            min_elevation: 900.0,
            near_z: 8.0,
            far_z: 1_952.897_196_030_780_6,
        },
        // Ground below sea level: GL JS's own test of a negative floor.
        Case {
            size: Size::new(500, 500),
            center: (0.0, 0.0),
            center_altitude: 0.0,
            zoom: 11.46,
            pitch: 60.0,
            top_padding: 0.0,
            min_elevation: -455.5,
            near_z: 10.0,
            far_z: 1_870.705_545_073_004,
        },
        // Past the horizon pitch, with the center moved down the view.
        Case {
            size: Size::new(512, 384),
            center: (27.9, 86.9),
            center_altitude: 300.0,
            zoom: 13.0,
            pitch: 85.0,
            top_padding: 120.0,
            min_elevation: 250.0,
            near_z: 7.68,
            far_z: 18_540.835_075_836_305,
        },
    ];
    for case in cases {
        let mut state = MapView {
            camera: Camera {
                center: LatLng::new(case.center.0, case.center.1).unwrap(),
                center_altitude: case.center_altitude,
                padding: EdgeInsets::new(case.top_padding, 0.0, 0.0, 0.0),
                zoom: case.zoom,
                bearing: 20.0,
                pitch: case.pitch,
                fov: mln_math::rad2deg(DEFAULT_FOV),
                ..Camera::default()
            },
            extent: LogicalExtent::new(case.size, 1.0),
            projection_mode: ProjectionMode::default(),
            viewport: Viewport::default(),
            projection: ProjectionDefinition::default(),
        }
        .to_state()
        .unwrap();
        state.set_terrain_min_elevation(Some(case.min_elevation));
        let view = state.view();
        assert_eq!(case.near_z, view.near_z());
        let far_z = view.far_z();
        assert!((far_z - case.far_z).abs() <= case.far_z * 1e-12, "{far_z} vs {}", case.far_z);
        assert_eq!(view.projection_matrix_with(view.near_z(), false), view.projection_matrix());
    }
}

/// The camera's distance to sea level is GL JS's (`cameraToSeaLevelDistance`
/// in `mercator_camera_math.ts`), at least half the distance to the center:
/// for a center above, at and below sea level, and past the horizon's pitch.
#[test]
fn the_camera_to_sea_level_distance_is_gl_js_s() {
    let (latitude, zoom) = (46.5, 12.0);
    // GL JS's `pixelsPerMeter`: the world's pixels over the circumference at
    // the latitude.
    let pixels_per_meter =
        512.0 * 2f64.powf(zoom) / (2.0 * std::f64::consts::PI * 6_371_008.8 * mln_math::deg2rad(latitude).cos());
    for (pitch, altitude) in [(60.0, 1200.0), (60.0, 0.0), (60.0, -400.0), (60.0, -30_000.0), (89.9, 1200.0)] {
        let mut state = pitched(pitch);
        state.set_lat_lng_zoom(&LatLng::new(latitude, 7.9).unwrap(), zoom);
        state.set_center_altitude(altitude);
        let view = state.view();
        let camera_to_center = view.camera_to_center_distance();
        let limited_pitch = mln_math::deg2rad(f64::min(pitch, 89.25));
        let expected =
            f64::max(camera_to_center / 2.0, camera_to_center + altitude * pixels_per_meter / limited_pitch.cos());
        let distance = view.camera_to_sea_level_distance();
        assert!((distance - expected).abs() <= expected * 1e-12, "{distance} vs {expected} at {pitch}°, {altitude} m");
        if altitude == 0.0 {
            assert_eq!(camera_to_center, distance);
        }
        if altitude == -30_000.0 {
            assert_eq!(camera_to_center / 2.0, distance, "at least half the distance to the center");
        }
    }
}

/// With terrain, the far plane reaches ground 2 km below sea level up to
/// the top of the view, where the flat map's plane cuts it off.
#[test]
fn the_far_plane_reaches_sunken_ground() {
    let mut state = pitched(60.0);
    state.set_lat_lng_zoom(&LatLng::new(0.0, -60.0).unwrap(), 13.0);
    // The depths of the sunken ground north of the center that shows.
    let depths = |state: &TransformState| -> Vec<f64> {
        let proj_matrix = state.view().projection_matrix();
        (0..8500)
            .filter_map(|step| {
                let lat_lng = LatLng::new(f64::from(step) / 10_000.0, -60.0).unwrap();
                let point = Projection::project(&lat_lng, state.view().scale());
                let clip = proj_matrix * DVec4::new(point.x, point.y, -2000.0, 1.0);
                (clip.w > 0.0 && clip.y / clip.w <= 1.0).then_some(clip.z / clip.w)
            })
            .collect()
    };
    let flat = depths(&state);
    assert!(flat.iter().any(|depth| *depth > 1.0));
    state.set_terrain_min_elevation(Some(-2000.0));
    let terrain = depths(&state);
    assert!(!terrain.is_empty() && terrain.iter().all(|depth| *depth < 1.0));
}

// MARK: - The camera on the globe

/// The globe's radius in meters, which heights on it are measured against.
const GLOBE_RADIUS: f64 = 6_371_008.8;

/// A 512 by 512 view under `vertical-perspective` of latitude 46.5,
/// longitude 7.9 at zoom 14, at `pitch` degrees and `bearing`, the center
/// `center_altitude` meters up.
fn on_globe(pitch: f64, bearing: f64, center_altitude: f64) -> TransformState {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(46.5, 7.9).unwrap(),
            center_altitude,
            zoom: 14.0,
            pitch,
            bearing,
            fov: mln_math::rad2deg(DEFAULT_FOV),
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::new(mln_style_types::ProjectionType::VerticalPerspective),
    }
    .to_state()
    .unwrap();
    state.set_limits(CameraLimits { max_pitch: mln_util::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    assert!(state.view().is_globe());
    state
}

/// The globe's radius in pixels at the view's zoom and center.
fn globe_radius_px(view: &crate::View) -> f64 {
    crate::VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(view.scale()),
        view.center(mln_util::geo::WrapMode::Unwrapped).latitude(),
    )
}

/// The unit-sphere point of a point of [`crate::View::eye_position`]'s
/// units: a Web Mercator point at zoom 0 and a height.
fn on_sphere(point: DVec3) -> DVec3 {
    let world = mln_util::geometry::Point::new(point.x, point.y);
    let location =
        Projection::unproject(&world, 1.0 / mln_util::constants::TILE_SIZE_D, mln_util::geo::WrapMode::Unwrapped)
            .unwrap();
    crate::VerticalPerspectiveProjection::surface_vector(&location) * (1.0 + point.z / GLOBE_RADIUS)
}

/// GL JS's globe camera, with the center at sea level and the near plane
/// half a pixel from the eye: the matrix, the clipping plane and the
/// camera's position.
fn sea_level_globe(view: &crate::View) -> (glam::DMat4, DVec4, DVec3) {
    use glam::DMat4;
    let radius = globe_radius_px(view);
    let center = view.center(mln_util::geo::WrapMode::Unwrapped);
    let (lat, lng) = (mln_math::deg2rad(center.latitude()), mln_math::deg2rad(center.longitude()));
    let distance = view.camera_to_center_distance();
    let size = view.size();
    let projection = glam::dcamera::rh::proj::opengl::perspective(
        f64::from(view.fov() as f32),
        f64::from(size.width) / f64::from(size.height),
        0.5,
        distance + radius * 2.0,
    );
    let matrix = projection
        * DMat4::from_translation(DVec3::new(0.0, 0.0, -distance))
        * DMat4::from_rotation_z(view.roll())
        * DMat4::from_rotation_x(-view.pitch())
        * DMat4::from_rotation_z(-view.angle())
        * DMat4::from_translation(DVec3::new(0.0, 0.0, -radius))
        * DMat4::from_rotation_x(lat)
        * DMat4::from_rotation_y(-lng)
        * DMat4::from_scale(DVec3::splat(radius));

    let pitch = view.pitch();
    let to_b = distance / radius;
    let (to_a, a_to_c) = (pitch.sin() * to_b, pitch.cos() * to_b + 1.0);
    let to_c = (to_a * to_a + a_to_c * a_to_c).sqrt();
    let mut plane = DVec3::new(0.0, -to_a / to_c, a_to_c / to_c);
    plane = glam::DMat3::from_rotation_z(view.angle()) * plane;
    plane = glam::DMat3::from_rotation_x(-lat) * plane;
    plane = glam::DMat3::from_rotation_y(lng) * plane;
    let plane = plane.extend(-1.0 / to_c) / plane.length();

    let mut position = DVec3::new(0.0, 0.0, to_b);
    position = glam::DMat3::from_rotation_z(-view.roll()) * position;
    position = glam::DMat3::from_rotation_x(pitch) * position;
    position = glam::DMat3::from_rotation_z(view.angle()) * position;
    position += DVec3::Z;
    position = glam::DMat3::from_rotation_x(-lat) * position;
    position = glam::DMat3::from_rotation_y(lng) * position;
    (matrix, plane, position)
}

/// With the center at sea level and no terrain, the globe's matrices are
/// GL JS's sea-level camera's, to the bit; a center raised by its altitude
/// draws at the center of the view, and terrain moves the near plane out.
#[test]
fn the_globe_camera_looks_at_the_center_at_its_altitude() {
    let state = on_globe(60.0, 30.0, 0.0);
    let view = state.view();
    let (matrix, plane, position) = sea_level_globe(view);
    assert_eq!(matrix.to_cols_array(), view.globe_view_projection_matrix().to_cols_array());
    assert_eq!(plane.to_array(), view.globe_clipping_plane().to_array());
    assert_eq!(position.to_array(), view.globe_camera_position().to_array());

    for altitude in [-400.0, 1200.0, 4000.0] {
        let mut state = state.clone();
        state.set_center_altitude(altitude);
        let view = state.view();
        let center =
            crate::VerticalPerspectiveProjection::surface_vector(&view.center(mln_util::geo::WrapMode::Unwrapped))
                * (1.0 + altitude / GLOBE_RADIUS);
        let clip = view.globe_view_projection_matrix() * center.extend(1.0);
        assert!((clip.x / clip.w).abs() < 1e-9 && (clip.y / clip.w).abs() < 1e-9, "{altitude} m: {clip}");
        // The eye is the camera's distance from that center.
        let distance = (view.globe_camera_position() - center).length() * globe_radius_px(view);
        assert!((distance - view.camera_to_center_distance()).abs() < 1e-6, "{altitude} m: {distance}");
    }

    let mut terrain = state.clone();
    terrain.set_terrain_min_elevation(Some(0.0));
    let near = terrain.view().near_plane_points()[4];
    let eye = terrain.view().eye_position();
    let to_near = (on_sphere(near) - on_sphere(eye)).length() * globe_radius_px(terrain.view());
    assert!((to_near - terrain.view().near_z()).abs() < 1e-3, "the near plane {to_near} px from the eye");
}

/// On the globe the eye stands where the camera is, back from the center
/// at its altitude along the view, and its height is measured from the
/// sea-level sphere; the near plane's points are the near plane's distance
/// from the eye along the view.
#[test]
fn the_globe_eye_and_near_plane_are_the_cameras() {
    let mut state = on_globe(70.0, 0.0, 1500.0);
    state.set_terrain_min_elevation(Some(1000.0));
    let view = state.view();
    let radius = globe_radius_px(view);
    let distance = view.camera_to_center_distance() / radius;
    let pitch = mln_math::deg2rad(70.0);
    // Looking north, the eye is south of the center, on its meridian.
    let (south, up) = (pitch.sin() * distance, 1.0 + 1500.0 / GLOBE_RADIUS + pitch.cos() * distance);
    let center = view.center(mln_util::geo::WrapMode::Unwrapped);
    let under = LatLng::new(center.latitude() - mln_math::rad2deg(south.atan2(up)), center.longitude()).unwrap();
    let expected = Projection::project(&under, 1.0 / mln_util::constants::TILE_SIZE_D);
    let eye = view.eye_position();
    assert!((eye.x - expected.x).abs() < 1e-12 && (eye.y - expected.y).abs() < 1e-12, "{eye}");
    let height = (south.hypot(up) - 1.0) * GLOBE_RADIUS;
    assert!((eye.z - height).abs() < 1e-6, "the eye {} m up, not {height}", eye.z);

    let eye = on_sphere(eye);
    let raised_center = crate::VerticalPerspectiveProjection::surface_vector(&center) * (1.0 + 1500.0 / GLOBE_RADIUS);
    let forward = (raised_center - eye).normalize();
    let points = view.near_plane_points();
    let middle = on_sphere(points[4]) - eye;
    assert!((middle - forward * view.near_z() / radius).length() * radius < 1e-3, "{middle}");
    for point in points {
        let along = (on_sphere(point) - eye).dot(forward) * radius;
        assert!((along - view.near_z()).abs() < 1e-3, "{point} is {along} px along the view");
    }
}

/// On the globe, a location at a height converts to the screen and back,
/// raised along its vertical, away from where it draws at sea level: within
/// a hundredth of a pixel wherever the line of sight meets the sphere at
/// that height, and at height 0 as the sea-level conversions, to the bit.
#[test]
fn globe_conversions_at_a_height_undo_each_other() {
    use mln_util::geo::{ScreenCoordinate, WrapMode};

    let state = on_globe(60.0, 20.0, 1000.0);
    let view = state.view();
    for row in 0..=8 {
        for column in 0..=8 {
            let pixel = ScreenCoordinate::new(64.0 * f64::from(column), 30.0 * f64::from(row));
            let flat = view.screen_coordinate_to_lat_lng(&pixel, WrapMode::Unwrapped).unwrap();
            let at = view.screen_coordinate_to_lat_lng_at(&pixel, 0.0, WrapMode::Unwrapped).unwrap();
            assert_eq!(flat, at);
            assert_eq!(view.lat_lng_to_screen_coordinate(&flat), view.lat_lng_to_screen_coordinate_at(&at, 0.0));
            for height in [-300.0, 1000.0, 2500.0] {
                let location = view.screen_coordinate_to_lat_lng_at(&pixel, height, WrapMode::Unwrapped).unwrap();
                let back = view.lat_lng_to_screen_coordinate_at(&location, height);
                let off = (back.x - pixel.x).hypot(back.y - pixel.y);
                assert!(off < 0.01, "{height} m: {pixel:?} comes back {off} px off");
                // At sea level the location draws elsewhere, by its height.
                let sea = view.lat_lng_to_screen_coordinate(&location);
                let parallax = (sea.x - pixel.x).hypot(sea.y - pixel.y);
                assert!(parallax > 10.0, "{height} m: {pixel:?} is {parallax} px from where it draws at sea level");
            }
        }
    }
}

/// On the globe, moving a location on the sphere at a height to a point
/// puts it there at that height, with the zoom, the angles and the center's
/// altitude kept.
#[test]
fn moving_a_location_on_a_sphere_puts_it_at_the_point() {
    use mln_util::geo::{ScreenCoordinate, WrapMode};

    let state = on_globe(55.0, 0.0, 400.0);
    let (from, to) = (ScreenCoordinate::new(140.0, 120.0), ScreenCoordinate::new(380.0, 200.0));
    for height in [0.0, 400.0, 1500.0] {
        let mut moved = state.clone();
        let location = moved.view().screen_coordinate_to_lat_lng_at(&from, height, WrapMode::Unwrapped).unwrap();
        moved.move_lat_lng_at(&location, &to, height).unwrap();
        // Where the location, raised along its vertical, draws.
        let raised = crate::VerticalPerspectiveProjection::surface_vector(&location) * (1.0 + height / GLOBE_RADIUS);
        let clip = moved.view().globe_view_projection_matrix() * raised.extend(1.0);
        let landed = ((clip.x / clip.w + 1.0) * 256.0, (clip.y / clip.w + 1.0) * 256.0);
        assert!((landed.0 - to.x).hypot(landed.1 - to.y) < 1e-2, "{height} m: {landed:?}");
        let (before, after) = (state.view(), moved.view());
        assert_eq!(
            (before.angle(), before.pitch(), before.center_altitude()),
            (after.angle(), after.pitch(), after.center_altitude())
        );
    }
}

/// On the globe at zoom 14, raising the eye over a ridge 300 m above it
/// clears the ridge, keeping the center, its altitude and the bearing, and
/// the ground under the eye.
#[test]
fn raising_the_eye_on_the_globe_clears_a_ridge() {
    use mln_util::geo::WrapMode;

    let mut state = on_globe(70.0, 40.0, 1000.0);
    state.set_terrain_min_elevation(Some(800.0));
    let before = state.view().clone();
    let eye = before.eye_position();
    let ridge = eye.z + 300.0;
    assert!(state.raise_eye(ridge - eye.z));
    let raised = state.view();
    let raised_eye = raised.eye_position();
    assert!((raised_eye.z - ridge).abs() < 0.01, "the eye {} m up, the ridge {ridge} m", raised_eye.z);
    let center = |view: &crate::View| view.center(WrapMode::Unwrapped);
    assert!((center(raised).latitude() - center(&before).latitude()).abs() < 1e-9);
    assert!((center(raised).longitude() - center(&before).longitude()).abs() < 1e-9);
    assert_eq!((1000.0, before.angle()), (raised.center_altitude(), raised.angle()));
    let under = |eye: DVec3| on_sphere(DVec3::new(eye.x, eye.y, 0.0));
    let moved = (under(raised_eye) - under(eye)).length() * GLOBE_RADIUS;
    assert!(moved < 0.5, "the ground under the eye moved {moved} m");
}

/// On the globe at zoom 14, moving the center along the view onto a point
/// 1000 m up on its line of sight puts the center there, keeps the pitch
/// and the bearing, and moves the eye less than a meter.
#[test]
fn recentering_on_the_globe_keeps_the_eye() {
    use mln_util::geo::WrapMode;

    let mut state = on_globe(60.0, 30.0, 0.0);
    let before = state.view().clone();
    let eye = before.globe_camera_position();
    let center = crate::VerticalPerspectiveProjection::surface_vector(&before.center(WrapMode::Unwrapped));
    let forward = (center - eye).normalize();
    // Where the center's line of sight comes down to 1000 m.
    let radius = 1.0 + 1000.0 / GLOBE_RADIUS;
    let b = eye.dot(forward);
    let t = -b - (b * b - eye.length_squared() + radius * radius).sqrt();
    let point = eye + forward * t;
    let location = crate::VerticalPerspectiveProjection::surface_vector_to_lat_lng(point.normalize()).unwrap();
    let world = Projection::project(&location, 1.0 / mln_util::constants::TILE_SIZE_D);
    let hit = crate::GroundHit { world: glam::DVec2::new(world.x, world.y), height: 1000.0 };

    assert!(state.recenter_on(&hit));
    let view = state.view();
    assert_eq!((before.pitch(), before.angle()), (view.pitch(), view.angle()));
    let moved = (view.globe_camera_position() - eye).length() * GLOBE_RADIUS;
    assert!(moved < 1.0, "the eye moved {moved} m");
    assert!((view.center_altitude() - 1000.0).abs() < 0.01, "the center {} m up", view.center_altitude());
    let off = (crate::VerticalPerspectiveProjection::surface_vector(&view.center(WrapMode::Unwrapped))
        - point.normalize())
    .length()
        * GLOBE_RADIUS;
    assert!(off < 0.01, "the center {off} m from the point");
    assert!(view.zoom() > before.zoom() + 0.1, "{}", view.zoom());
}
