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

/// Without terrain the planes and matrices are those the view had before it
/// knew of terrain, to the bit, and a terrain that goes away restores them.
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
                0.007_508_877_260_565_23,
                -0.004_520_344_110_860_268,
                -0.091_007_877_558_885_29,
                -0.090_406_882_217_205_36,
                -3_028_316.878_903_641,
                1_147_156.446_909_028_3,
                298.848_628_475_551_95,
                298.868_496_274_289_27,
            ],
            [0.0, 0.0, -0.109_624_258_666_032_52, 298.233_205_214_500_34],
        ),
        (
            alpine(true),
            54_478.153_105_644_335,
            [
                1.784_059_983_608_735_4,
                0.661_384_306_806_117_8,
                0.326_202_383_497_395_66,
                0.326_190_408_187_028_27,
                0.833_051_235_020_185_3,
                -0.450_162_381_905_947_7,
                -0.896_233_682_831_371_7,
                -0.896_200_780_936_546_3,
                -0.024_515_372_283_410_004,
                0.261_188_125_549_333_3,
                -0.027_173_570_463_330_236,
                -0.027_172_572_886_498_946,
                -3_057_312.553_228_450_5,
                -463_638.868_611_956_83,
                366_351.995_260_439_06,
                366_340.545_969_311_15,
            ],
            [0.326_537_870_775_438_83, -0.897_155_426_552_309_5, -0.027_201_517_491_465_696, 366_672.746_155_253_44],
        ),
    ];
    for (mut state, far_z, projection, clipped_z) in expected {
        let far_z: f64 = far_z;
        for _ in 0..2 {
            let view = state.view();
            assert_eq!(1.0, view.near_z());
            assert_eq!(far_z.to_bits(), view.far_z().to_bits());
            assert_eq!(projection.map(f64::to_bits), view.projection_matrix().to_cols_array().map(f64::to_bits));
            let near_clipped = f64::from((0.1 * view.camera_to_center_distance()) as u16);
            let clipped = view.projection_matrix_with(near_clipped, false).row(2).to_array();
            assert_eq!(clipped_z.map(f64::to_bits), clipped.map(f64::to_bits));

            let before = view.clone();
            state.set_terrain_min_elevation(Some(-120.0));
            assert_ne!(before.far_z(), state.view().far_z());
            state.set_terrain_min_elevation(None);
            assert_eq!(before, *state.view());
        }
    }
}

/// With terrain, the near and far planes are GL JS's (`calculateNearFarZ`
/// in `mercator_camera_math.ts)`: the numbers come from that function run on
/// the same cameras, with mln-rs's earth radius for the pixels per meter. A
/// copy of it with GL JS's radius gives GL JS's own test expectations.
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
            far_z: 1_952.317_066_220_289_5,
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
            far_z: 1_870.617_879_146_915_8,
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
            far_z: 18_532.242_194_604_092,
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
