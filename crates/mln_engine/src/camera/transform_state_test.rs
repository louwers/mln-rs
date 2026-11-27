use glam::{DQuat, DVec3, DVec4};
use mln_style::constants::LATITUDE_MAX;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_style::types::ProjectionDefinition;

use super::TransformState;
use crate::camera::constants::DEFAULT_FOV;
use crate::camera::{Camera, CameraLimits, LogicalExtent, MapView, ProjectionMode, Viewport};
use crate::geo::projection::Projection;
use crate::geo::world::WorldMapping;

/// A 512 by 512 view at `pitch` degrees, looking north.
fn pitched(pitch: f64) -> TransformState {
    MapView {
        camera: Camera {
            center: LatLng::new(0.0, -60.0).unwrap(),
            zoom: 5.0,
            pitch,
            fov: mln_style::math::rad2deg(DEFAULT_FOV),
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state(&WorldMapping::Mercator)
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

/// At high pitch the far plane ends the map at the sky's horizon, which the
/// sky draws from the middle of the view, so the padding moves it up the
/// view from the padded center or down: the ground drawn below that row is
/// nearer than the far plane, and the ground above it is past it. The map
/// never reaches past 89.25 degrees from straight down, which here stops
/// top padding short of the sky's horizon, and always reaches its center,
/// though bottom padding moves the sky's horizon below it. With terrain,
/// the far plane ends sea-level ground there too.
#[test]
fn padding_moves_the_map_s_end_with_the_sky_s_horizon() {
    use crate::geo::EdgeInsets;

    for (top, bottom) in [(0.0, 40.0), (0.0, 80.0), (0.0, 160.0), (40.0, 0.0)] {
        let mut state = pitched(85.0);
        state.set_padding(&EdgeInsets::new(top, 0.0, bottom, 0.0));
        let view = state.view();
        let center_row = 256.0 + (top - bottom) / 2.0;
        let farthest = view.camera_to_center_distance() * mln_style::math::deg2rad(89.25 - 85.0).tan();
        let up = center_row - (256.0 - view.mercator_horizon());
        let end_row = center_row - mln_style::math::clamp_f64(up, 0.0, farthest);
        for min_elevation in [None, Some(0.0)] {
            state.set_terrain_min_elevation(min_elevation);
            let ground = meridian(&state);
            let case = format!("padding {top} top, {bottom} bottom, {min_elevation:?}");
            assert!(ground.iter().filter(|(row, _)| *row > end_row + 1.0).all(|(_, depth)| *depth < 1.0), "{case}");
            let past: Vec<_> = ground.iter().filter(|(row, _)| *row < end_row - 1.0).collect();
            assert_ne!(past.len(), 0, "{case}");
            assert!(past.iter().all(|(row, depth)| *row > 0.0 && *depth > 1.0), "{case}: ground past row {end_row}");
        }
    }
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
        let phi = mln_style::math::deg2rad(latitude);
        let globe = phi.tan().asinh();
        let f = phi.sin();
        let mercator = 0.5 * ((1.0 + f) / (1.0 - f)).ln();
        assert!((globe - mercator).abs() <= 1e-12, "{latitude}: {globe} vs {mercator}");
    }
}

#[test]
fn a_center_too_far_to_place_leaves_the_camera_unchanged() {
    use mln_style::geo::WrapMode;

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
    use mln_style::geo::WrapMode;
    use mln_style::types::ProjectionType;

    use crate::camera::FreeCameraOptions;
    use crate::geo::{EdgeInsets, ScreenCoordinate};

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
                let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
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
                }));
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
    assert_eq!(crate::camera::constants::PITCH_MAX, state.limits().max_pitch);
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
    use crate::geo::EdgeInsets;

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
    .to_state(&WorldMapping::Mercator)
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
            11_984.201_437_014_68,
            [
                1.784_059_983_608_735_4,
                0.661_384_306_806_117_8,
                0.326_244_849_466_331_03,
                0.326_190_408_187_028_27,
                0.833_051_235_020_185_3,
                -0.450_162_381_905_947_7,
                -0.896_350_357_122_117,
                -0.896_200_780_936_546_3,
                -0.024_527_372_501_201_117,
                0.261_315_976_530_130_6,
                -0.027_190_411_128_499_33,
                -0.027_185_873_797_813_42,
                -3_057_312.553_228_450_5,
                -463_638.868_611_956_83,
                366_399.688_152_04,
                366_340.545_969_311_15,
            ],
            [0.327_772_902_950_878_3, -0.900_548_649_566_629_4, -0.027_317_764_564_236_752, 368_059.686_218_946_76],
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
                fov: mln_style::math::rad2deg(DEFAULT_FOV),
                ..Camera::default()
            },
            extent: LogicalExtent::new(Size::new(512, 512), 1.0),
            projection_mode: ProjectionMode::default(),
            viewport: Viewport::default(),
            projection: ProjectionDefinition::default(),
        }
        .to_state(&WorldMapping::Mercator)
        .unwrap();
        let view = state.view();
        let center = Projection::project(&view.center(mln_style::geo::WrapMode::Unwrapped), view.scale());
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
    use mln_style::geo::WrapMode;

    use crate::geo::ScreenCoordinate;

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

/// Where `view`'s projection matrix draws `lat_lng`, `height` meters above
/// sea level, measured from the bottom edge (y up).
fn drawn_at(view: &crate::camera::View, lat_lng: &LatLng, height: f64) -> crate::geo::ScreenCoordinate {
    let point = Projection::project(lat_lng, view.scale());
    let clip = view.projection_matrix() * DVec4::new(point.x, point.y, height, 1.0);
    let size = view.size();
    crate::geo::ScreenCoordinate::new(
        f64::midpoint(clip.x / clip.w, 1.0) * f64::from(size.width),
        f64::midpoint(clip.y / clip.w, 1.0) * f64::from(size.height),
    )
}

/// The line of sight through a point meets the plane at a height where the
/// projection matrix draws the location it reads there, at that height; at
/// sea level the location is `screen_coordinate_to_lat_lng`'s, to the bit.
#[test]
fn a_point_reads_the_location_its_line_of_sight_meets_on_a_plane() {
    use mln_style::geo::WrapMode;

    use crate::geo::ScreenCoordinate;

    let mut state = pitched(55.0);
    state.set_lat_lng_zoom(&LatLng::new(30.0, 20.0).unwrap(), 12.0);
    state.set_center_altitude(400.0);
    let view = state.view();
    let point = ScreenCoordinate::new(140.0, 120.0);
    for height in [-300.0, 0.0, 400.0, 1500.0] {
        let location = view.screen_coordinate_to_lat_lng_at(&point, height, WrapMode::Unwrapped).unwrap();
        let drawn = drawn_at(view, &location, height);
        assert!((drawn.x - point.x).hypot(drawn.y - point.y) < 1e-6, "{height} m: {drawn:?}");
    }
    assert_eq!(
        view.screen_coordinate_to_lat_lng(&point, WrapMode::Unwrapped),
        view.screen_coordinate_to_lat_lng_at(&point, 0.0, WrapMode::Unwrapped)
    );
}

/// Moving a location on a plane above sea level to a point puts it there,
/// at that height, with the zoom, the angles and the center's altitude
/// kept; at sea level it is `move_lat_lng`, to the bit.
#[test]
fn moving_a_location_on_a_plane_puts_it_at_the_point() {
    use mln_style::geo::WrapMode;

    use crate::geo::ScreenCoordinate;

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
        let landed = drawn_at(moved.view(), &location, height);
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

/// The line of sight through the view's center descends through the center
/// at its altitude, from the near plane to the far plane; through a point
/// above the horizon of a steep view it rises.
#[test]
fn the_line_of_sight_runs_from_the_near_plane_to_the_far_plane() {
    use mln_style::geo::WrapMode;

    use crate::geo::ScreenCoordinate;

    let mut state = pitched(60.0);
    state.set_center_altitude(700.0);
    let view = state.view();
    let (near, far) = view.line_of_sight(&ScreenCoordinate::new(256.0, 256.0)).unwrap();
    assert!(far.z < near.z && far.z < 700.0 && 700.0 < near.z, "{near} {far}");
    let center = Projection::project(&view.center(WrapMode::Unwrapped), 1.0 / mln_style::constants::TILE_SIZE_D);
    let along = (700.0 - near.z) / (far.z - near.z);
    let at = near + (far - near) * along;
    assert!((at.x - center.x).abs() < 1e-12 && (at.y - center.y).abs() < 1e-12, "{at} against {center:?}");

    let steep = pitched(85.0);
    let (near, far) = steep.view().line_of_sight(&ScreenCoordinate::new(256.0, 500.0)).unwrap();
    assert!(far.z >= near.z, "{near} {far}");
}

/// With terrain, the near and far planes are GL JS's (`calculateNearFarZ`
/// in `mercator_camera_math.ts`): the numbers come from that function run on
/// the same cameras, with the WGS84 radius for the pixels per meter. With
/// GL JS's radius, the camera below sea level gives GL JS's own test's far
/// plane.
#[test]
fn terrain_planes_are_gl_js_planes() {
    struct Case {
        size: Size,
        center: (f64, f64),
        center_altitude: f64,
        zoom: f64,
        pitch: f64,
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
            min_elevation: -455.5,
            near_z: 10.0,
            far_z: 1_870.617_879_146_915_8,
        },
    ];
    for case in cases {
        let mut state = MapView {
            camera: Camera {
                center: LatLng::new(case.center.0, case.center.1).unwrap(),
                center_altitude: case.center_altitude,
                zoom: case.zoom,
                bearing: 20.0,
                pitch: case.pitch,
                fov: mln_style::math::rad2deg(DEFAULT_FOV),
                ..Camera::default()
            },
            extent: LogicalExtent::new(case.size, 1.0),
            projection_mode: ProjectionMode::default(),
            viewport: Viewport::default(),
            projection: ProjectionDefinition::default(),
        }
        .to_state(&WorldMapping::Mercator)
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
    // the latitude, on the WGS84 radius that heights scale on.
    let pixels_per_meter =
        512.0 * 2f64.powf(zoom) / (2.0 * std::f64::consts::PI * 6_378_137.0 * mln_style::math::deg2rad(latitude).cos());
    for (pitch, altitude) in [(60.0, 1200.0), (60.0, 0.0), (60.0, -400.0), (60.0, -30_000.0), (89.9, 1200.0)] {
        let mut state = pitched(pitch);
        state.set_lat_lng_zoom(&LatLng::new(latitude, 7.9).unwrap(), zoom);
        state.set_center_altitude(altitude);
        let view = state.view();
        let camera_to_center = view.camera_to_center_distance();
        let limited_pitch = mln_style::math::deg2rad(f64::min(pitch, 89.25));
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

/// A rolled view reaches as far over terrain as its rotated corners do: in a
/// portrait view rolled 90 degrees, the ground at the view's sides falls
/// inside the depth range.
#[test]
fn the_terrain_far_plane_reaches_a_rolled_views_sides() {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(0.0, 0.0).unwrap(),
            zoom: 12.0,
            pitch: 60.0,
            roll: 90.0,
            fov: 60.0,
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(250, 500), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state(&WorldMapping::Mercator)
    .unwrap();
    state.set_terrain_min_elevation(Some(0.0));
    let view = state.view();
    let proj_matrix = view.projection_matrix();
    let mut seen = 0;
    for row in -150..=150 {
        for column in -150..=150 {
            let lat_lng = LatLng::new(f64::from(row) / 1000.0, f64::from(column) / 1000.0).unwrap();
            let point = Projection::project(&lat_lng, view.scale());
            let clip = proj_matrix * DVec4::new(point.x, point.y, 0.0, 1.0);
            if clip.w > 0.0 && (clip.x / clip.w).abs() <= 1.0 && (clip.y / clip.w).abs() <= 1.0 {
                seen += 1;
                assert!(clip.z / clip.w < 1.0, "{lat_lng:?} is past the far plane");
            }
        }
    }
    assert!(seen > 1000, "{seen} points in view");
}

/// A 512 by 512 view of latitude 52.5, longitude 13.4 at `zoom`, pitched
/// 60 degrees and turned 20, its center `center_altitude` meters up, under
/// `projection`.
fn raised(projection: ProjectionDefinition, zoom: f64, center_altitude: f64) -> TransformState {
    MapView {
        camera: Camera {
            center: LatLng::new(52.5, 13.4).unwrap(),
            center_altitude,
            zoom,
            pitch: 60.0,
            bearing: 20.0,
            fov: mln_style::math::rad2deg(DEFAULT_FOV),
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection,
    }
    .to_state(&WorldMapping::Mercator)
    .unwrap()
}

/// The globe's camera looks at the center raised by its altitude: the
/// center, as high as that, draws at the center of the view, and the eye is
/// the camera's distance from it.
#[test]
fn the_globe_draws_the_center_at_its_altitude_at_the_center() {
    use mln_style::geo::WrapMode;
    use mln_style::types::ProjectionType;

    use crate::camera::VerticalPerspectiveProjection;

    const GLOBE_RADIUS: f64 = 6_371_008.8;
    let globe = ProjectionDefinition::new(ProjectionType::VerticalPerspective);
    for (zoom, center_altitude) in [(3.0, 9000.0), (8.0, -400.0), (14.0, 3000.0)] {
        let state = raised(globe, zoom, center_altitude);
        let view = state.view();
        let center = VerticalPerspectiveProjection::surface_vector(&view.center(WrapMode::Unwrapped))
            * (1.0 + center_altitude / GLOBE_RADIUS);
        let clip = view.globe_view_projection_matrix() * center.extend(1.0);
        assert!((clip.x / clip.w).abs() < 1e-9 && (clip.y / clip.w).abs() < 1e-9, "zoom {zoom}: {clip}");
        let radius = VerticalPerspectiveProjection::globe_radius_pixels(
            Projection::world_size(view.scale()),
            view.center(WrapMode::Unwrapped).latitude(),
        );
        let distance = (view.globe_camera_position() - center).length() * radius;
        assert!((distance - view.camera_to_center_distance()).abs() < 1e-6, "zoom {zoom}: {distance}");
    }
}

/// On the globe, a location at a height converts to the screen and back,
/// raised along its vertical, away from where it draws at sea level: within
/// a hundredth of a pixel wherever the line of sight meets the sphere at
/// that height, and at height 0 as the sea-level conversions, to the bit.
#[test]
fn globe_conversions_at_a_height_undo_each_other() {
    use mln_style::geo::WrapMode;
    use mln_style::types::ProjectionType;

    use crate::geo::ScreenCoordinate;

    let state = raised(ProjectionDefinition::new(ProjectionType::VerticalPerspective), 14.0, 1000.0);
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
    use mln_style::geo::WrapMode;
    use mln_style::types::ProjectionType;

    use crate::camera::VerticalPerspectiveProjection;
    use crate::geo::ScreenCoordinate;

    let state = raised(ProjectionDefinition::new(ProjectionType::VerticalPerspective), 14.0, 400.0);
    let (from, to) = (ScreenCoordinate::new(140.0, 120.0), ScreenCoordinate::new(380.0, 200.0));
    for height in [0.0, 400.0, 1500.0] {
        let mut moved = state.clone();
        let location = moved.view().screen_coordinate_to_lat_lng_at(&from, height, WrapMode::Unwrapped).unwrap();
        moved.move_lat_lng_at(&location, &to, height).unwrap();
        // Where the location, raised along its vertical, draws.
        let raised = VerticalPerspectiveProjection::surface_vector(&location)
            * (1.0 + height / VerticalPerspectiveProjection::GLOBE_RADIUS_METERS);
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

/// At zoom 12, where the `globe` preset hands over to Mercator, the globe
/// draws the ground under a raised center where Mercator does, as it does
/// under a center at sea level: the handover doesn't slide the map by the
/// center's altitude.
#[test]
fn the_globe_hands_over_to_mercator_with_the_center_raised() {
    use mln_style::types::ProjectionType;

    use crate::geo::ScreenCoordinate;

    let globe = ProjectionDefinition::new(ProjectionType::VerticalPerspective);
    let farthest_apart = |center_altitude: f64| {
        let mercator = raised(ProjectionDefinition::default(), 12.0, center_altitude);
        let globe = raised(globe, 12.0, center_altitude);
        let mut farthest: f64 = 0.0;
        for row in 0..=8 {
            for column in 0..=8 {
                // The ground that Mercator draws across the lower half of the
                // view, below its horizon.
                let pixel = ScreenCoordinate::new(64.0 * f64::from(column), 32.0 * f64::from(row));
                let location =
                    mercator.view().screen_coordinate_to_lat_lng(&pixel, mln_style::geo::WrapMode::Unwrapped).unwrap();
                let drawn = globe.view().lat_lng_to_screen_coordinate(&location);
                farthest = farthest.max((drawn.x - pixel.x).hypot(drawn.y - pixel.y));
            }
        }
        farthest
    };
    let at_sea_level = farthest_apart(0.0);
    let raised = farthest_apart(3000.0);
    assert!(at_sea_level < 2.0, "{at_sea_level} px apart at sea level");
    assert!(raised < at_sea_level + 1.0, "{raised} px apart with the center 3000 m up");
}

// MARK: - Raising the eye and recentering

/// The eye stands back from the center along the line of sight, and the
/// near plane's middle point is on that line, the near plane's distance
/// further. Raising the eye keeps the ground under it and the center it
/// looks at, with its altitude, the bearing and the roll.
#[test]
fn raising_the_eye_keeps_its_place_and_the_center() {
    use mln_style::geo::WrapMode;

    let mut state = pitched(70.0);
    state.set_limits(CameraLimits { max_pitch: crate::camera::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    state.set_angle(0.5);
    state.set_roll(0.1);
    state.set_center_altitude(1000.0);
    state.set_terrain_min_elevation(Some(800.0));
    let view = state.view().clone();
    let meters_per_pixel = view.meters_per_pixel();
    let world_size = Projection::world_size(view.scale());
    let eye = view.eye_position();
    let rise = mln_style::math::deg2rad(70.0).cos() * view.camera_to_center_distance() * meters_per_pixel;
    assert!((eye.z - (1000.0 + rise)).abs() < 1e-6, "{eye}");
    let middle = view.near_plane_points()[4];
    let to_middle = DVec3::new(
        (middle.x - eye.x) * world_size,
        (middle.y - eye.y) * world_size,
        (middle.z - eye.z) / meters_per_pixel,
    );
    assert!((to_middle.length() - view.near_z()).abs() < 1e-6, "{to_middle}");
    assert!((to_middle.z / to_middle.length() + mln_style::math::deg2rad(70.0).cos()).abs() < 1e-9, "{to_middle}");

    assert!(state.raise_eye(500.0));
    let raised = state.view();
    let center = |view: &crate::camera::View| view.center(WrapMode::Unwrapped);
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
    state.set_limits(CameraLimits { max_pitch: crate::camera::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    state.set_pitch(mln_style::math::deg2rad(95.0));
    let eye = state.view().eye_position();
    assert!(eye.z < 0.0, "{eye}");
    assert!(state.raise_eye(-eye.z));
    assert!((state.view().pitch() - mln_style::math::deg2rad(90.0)).abs() < 1e-12);
    let zoom = 5.0 + (1.0 / mln_style::math::deg2rad(95.0).sin()).log2();
    assert!((state.view().zoom() - zoom).abs() < 1e-12, "{}", state.view().zoom());
}

/// A view at latitude 45, zoom 14 and pitch 60, turned, with the center at
/// sea level, and the point 1000 m up on its center's line of sight, where
/// ground that rises in front of the center would meet it.
fn looking_at_a_rise() -> (TransformState, crate::camera::GroundHit) {
    use mln_style::geo::WrapMode;

    let mut state = pitched(60.0);
    state.set_angle(0.5);
    state.set_lat_lng_zoom(&LatLng::new(45.0, 10.0).unwrap(), 14.0);
    let view = state.view();
    let center = view.lat_lng_to_screen_coordinate(&view.center(WrapMode::Unwrapped));
    let (near, far) = view.line_of_sight(&center).unwrap();
    let point = near + (far - near) * ((1000.0 - near.z) / (far.z - near.z));
    (state, crate::camera::GroundHit { world: point.truncate(), height: 1000.0 })
}

/// Meters along the ground at latitude 45 in a unit of Web Mercator at zoom 0.
fn meters_at_45() -> f64 {
    Projection::meters_per_pixel_at_latitude(45.0, 0.0) * mln_style::constants::TILE_SIZE_D
}

/// Moving the center along the view onto a point of its line of sight keeps
/// the eye, the pitch and the bearing, and zooms in to the nearer center,
/// which stands where the view at the new latitude reaches the point's
/// height, off it by far less than a meter.
#[test]
fn recentering_keeps_the_eye_and_moves_the_center_along_the_view() {
    use mln_style::geo::WrapMode;

    let (mut state, hit) = looking_at_a_rise();
    let before = state.view().clone();
    assert!(state.recenter_on(&hit));
    let view = state.view();
    assert_eq!((before.pitch(), before.angle()), (view.pitch(), view.angle()));
    let moved = view.eye_position() - before.eye_position();
    let moved = DVec3::new(moved.x * meters_at_45(), moved.y * meters_at_45(), moved.z);
    assert!(moved.length() < 1e-5, "the eye moved {moved} m");
    assert!(view.zoom() > before.zoom() + 0.1, "{}", view.zoom());
    let center = Projection::project(&view.center(WrapMode::Unwrapped), 1.0 / mln_style::constants::TILE_SIZE_D);
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
    let behind = crate::camera::GroundHit { world: hit.world + (eye.truncate() - hit.world) * 2.0, height: hit.height };
    assert!(!state.recenter_on(&behind));
    assert_eq!(before, state);
    state.set_limits(CameraLimits { max_zoom: 14.0, ..*state.limits() }).unwrap();
    let before = state.clone();
    assert!(!state.recenter_on(&hit));
    assert_eq!(before, state);
}

/// A center the bounds would move leaves the state as it is: bounds that
/// hold the center but not the point.
#[test]
fn recentering_refuses_a_center_out_of_bounds() {
    use mln_style::geo::{LatLngBounds, WrapMode};

    let (mut state, hit) = looking_at_a_rise();
    let center = state.view().center(WrapMode::Unwrapped);
    let corner = |offset: f64| LatLng::new(center.latitude() + offset, center.longitude() + offset).unwrap();
    let bounds = LatLngBounds::hull(corner(-1e-3), corner(1e-3));
    state.set_limits(CameraLimits { bounds, ..*state.limits() }).unwrap();
    let before = state.clone();
    assert!(!state.recenter_on(&hit));
    assert_eq!(before, state);
}

// MARK: - Raising the eye and recentering on the globe

/// The globe's radius in meters, which heights on it are measured against.
const GLOBE_RADIUS: f64 = crate::camera::VerticalPerspectiveProjection::GLOBE_RADIUS_METERS;

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
            fov: mln_style::math::rad2deg(DEFAULT_FOV),
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(512, 512), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::new(mln_style::types::ProjectionType::VerticalPerspective),
    }
    .to_state(&WorldMapping::Mercator)
    .unwrap();
    state.set_limits(CameraLimits { max_pitch: crate::camera::constants::PITCH_MAX, ..*state.limits() }).unwrap();
    assert!(state.view().is_globe());
    state
}

/// The globe's radius in pixels at the view's zoom and center.
fn globe_radius_px(view: &crate::camera::View) -> f64 {
    crate::camera::VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(view.scale()),
        view.center(mln_style::geo::WrapMode::Unwrapped).latitude(),
    )
}

/// The unit-sphere point of a point of [`crate::camera::View::eye_position`]'s
/// units: a Web Mercator point at zoom 0 and a height.
fn on_sphere(point: DVec3) -> DVec3 {
    let world = mln_style::geometry::Point::new(point.x, point.y);
    let location =
        Projection::unproject(&world, 1.0 / mln_style::constants::TILE_SIZE_D, mln_style::geo::WrapMode::Unwrapped)
            .unwrap();
    crate::camera::VerticalPerspectiveProjection::surface_vector(&location) * (1.0 + point.z / GLOBE_RADIUS)
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
    let pitch = mln_style::math::deg2rad(70.0);
    // Looking north, the eye is south of the center, on its meridian.
    let (south, up) = (pitch.sin() * distance, 1.0 + 1500.0 / GLOBE_RADIUS + pitch.cos() * distance);
    let center = view.center(mln_style::geo::WrapMode::Unwrapped);
    let under = LatLng::new(center.latitude() - mln_style::math::rad2deg(south.atan2(up)), center.longitude()).unwrap();
    let expected = Projection::project(&under, 1.0 / mln_style::constants::TILE_SIZE_D);
    let eye = view.eye_position();
    assert!((eye.x - expected.x).abs() < 1e-12 && (eye.y - expected.y).abs() < 1e-12, "{eye}");
    let height = (south.hypot(up) - 1.0) * GLOBE_RADIUS;
    assert!((eye.z - height).abs() < 1e-6, "the eye {} m up, not {height}", eye.z);

    let eye = on_sphere(eye);
    let raised_center =
        crate::camera::VerticalPerspectiveProjection::surface_vector(&center) * (1.0 + 1500.0 / GLOBE_RADIUS);
    let forward = (raised_center - eye).normalize();
    let points = view.near_plane_points();
    let middle = on_sphere(points[4]) - eye;
    assert!((middle - forward * view.near_z() / radius).length() * radius < 1e-3, "{middle}");
    for point in points {
        let along = (on_sphere(point) - eye).dot(forward) * radius;
        assert!((along - view.near_z()).abs() < 1e-3, "{point} is {along} px along the view");
    }
}

/// On the globe at zoom 14, raising the eye over a ridge 300 m above it
/// clears the ridge, keeping the center, its altitude and the bearing, and
/// the ground under the eye.
#[test]
fn raising_the_eye_on_the_globe_clears_a_ridge() {
    use mln_style::geo::WrapMode;

    let mut state = on_globe(70.0, 40.0, 1000.0);
    state.set_terrain_min_elevation(Some(800.0));
    let before = state.view().clone();
    let eye = before.eye_position();
    let ridge = eye.z + 300.0;
    assert!(state.raise_eye(ridge - eye.z));
    let raised = state.view();
    let raised_eye = raised.eye_position();
    assert!((raised_eye.z - ridge).abs() < 0.01, "the eye {} m up, the ridge {ridge} m", raised_eye.z);
    let center = |view: &crate::camera::View| view.center(WrapMode::Unwrapped);
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
    use mln_style::geo::WrapMode;

    let mut state = on_globe(60.0, 30.0, 0.0);
    let before = state.view().clone();
    let eye = before.globe_camera_position();
    let center = crate::camera::VerticalPerspectiveProjection::surface_vector(&before.center(WrapMode::Unwrapped));
    let forward = (center - eye).normalize();
    // Where the center's line of sight comes down to 1000 m.
    let radius = 1.0 + 1000.0 / GLOBE_RADIUS;
    let b = eye.dot(forward);
    let t = -b - (b * b - eye.length_squared() + radius * radius).sqrt();
    let point = eye + forward * t;
    let location = crate::camera::VerticalPerspectiveProjection::surface_vector_to_lat_lng(point.normalize()).unwrap();
    let world = Projection::project(&location, 1.0 / mln_style::constants::TILE_SIZE_D);
    let hit = crate::camera::GroundHit { world: glam::DVec2::new(world.x, world.y), height: 1000.0 };

    assert!(state.recenter_on(&hit));
    let view = state.view();
    assert_eq!((before.pitch(), before.angle()), (view.pitch(), view.angle()));
    let moved = (view.globe_camera_position() - eye).length() * GLOBE_RADIUS;
    assert!(moved < 1.0, "the eye moved {moved} m");
    assert!((view.center_altitude() - 1000.0).abs() < 0.01, "the center {} m up", view.center_altitude());
    let off = (crate::camera::VerticalPerspectiveProjection::surface_vector(&view.center(WrapMode::Unwrapped))
        - point.normalize())
    .length()
        * GLOBE_RADIUS;
    assert!(off < 0.01, "the center {off} m from the point");
    assert!(view.zoom() > before.zoom() + 0.1, "{}", view.zoom());
}

/// A `width` by `height` view of the equator at zoom 12, `pitch` and `roll`
/// degrees, with a field of view of 60 degrees, `padding`, and terrain down
/// to sea level.
fn rolled(width: u32, height: u32, pitch: f64, roll: f64, padding: crate::geo::EdgeInsets) -> TransformState {
    let mut state = MapView {
        camera: Camera {
            center: LatLng::new(0.0, 0.0).unwrap(),
            padding,
            zoom: 12.0,
            pitch,
            roll,
            fov: 60.0,
            ..Camera::default()
        },
        extent: LogicalExtent::new(Size::new(width, height), 1.0),
        projection_mode: ProjectionMode::default(),
        viewport: Viewport::default(),
        projection: ProjectionDefinition::default(),
    }
    .to_state(&WorldMapping::Mercator)
    .unwrap();
    state.set_terrain_min_elevation(Some(0.0));
    state
}

/// The sea-level ground within a degree of the equator's center that
/// `state` shows below GL JS's horizon: each point's depth from the camera
/// in pixels and its clip z over w.
fn visible_sea_level(state: &TransformState) -> Vec<(f64, f64)> {
    let view = state.view();
    let proj_matrix = view.projection_matrix();
    // How far the ground at GL JS's horizon is from the camera along its
    // line of sight: the far plane need not reach past it.
    let pitch = view.pitch();
    let camera_height = view.camera_to_center_distance() * pitch.cos();
    let horizon_angle = pitch + (view.mercator_horizon() / view.camera_to_center_distance()).atan();
    let horizon_depth = camera_height * (horizon_angle - pitch).cos() / horizon_angle.cos();
    let mut ground = Vec::new();
    for row in -250..=250 {
        for column in -250..=250 {
            let lat_lng = LatLng::new(f64::from(row) / 250.0, f64::from(column) / 250.0).unwrap();
            let point = Projection::project(&lat_lng, view.scale());
            let clip = proj_matrix * DVec4::new(point.x, point.y, 0.0, 1.0);
            let in_view = clip.w > 0.0 && (clip.x / clip.w).abs() <= 1.0 && (clip.y / clip.w).abs() <= 1.0;
            if in_view && clip.w < horizon_depth * 0.999 {
                ground.push((clip.w, clip.z / clip.w));
            }
        }
    }
    ground
}

/// The far plane reaches the corner of a turned view that is farthest up
/// the map, with or without terrain: a roll, the viewport's flip and the
/// north orientation turn the view, and padding moves its center off the
/// middle on either axis.
#[test]
fn the_far_plane_reaches_a_turned_views_farthest_corner() {
    use crate::camera::mode::ViewportMode;
    use crate::camera::view::NorthOrientation;
    use crate::geo::EdgeInsets;

    let bottom = EdgeInsets::new(0.0, 0.0, 50.0, 0.0);
    let left = EdgeInsets::new(0.0, 200.0, 0.0, 0.0);
    let right = EdgeInsets::new(0.0, 0.0, 0.0, 200.0);
    let cases = [
        (90.0, bottom, NorthOrientation::Upwards, ViewportMode::Default),
        (90.0, left, NorthOrientation::Upwards, ViewportMode::Default),
        (-90.0, right, NorthOrientation::Upwards, ViewportMode::Default),
        (180.0, bottom, NorthOrientation::Upwards, ViewportMode::Default),
        (0.0, bottom, NorthOrientation::Upwards, ViewportMode::FlippedY),
        (0.0, bottom, NorthOrientation::Rightwards, ViewportMode::Default),
        (0.0, left, NorthOrientation::Leftwards, ViewportMode::Default),
        (30.0, right, NorthOrientation::Downwards, ViewportMode::FlippedY),
    ];
    for (roll, padding, orientation, viewport_mode) in cases {
        let mut state = rolled(1000, 500, 30.0, roll, padding);
        state.set_north_orientation(orientation);
        state.set_viewport_mode(viewport_mode);
        for min_elevation in [Some(0.0), None] {
            state.set_terrain_min_elevation(min_elevation);
            let ground = visible_sea_level(&state);
            assert!(ground.len() > 1000, "{} points in view", ground.len());
            let (depth, z) = ground.iter().copied().fold((0.0, 0.0), |a, b| if b.0 > a.0 { b } else { a });
            let case = format!("roll {roll}, {padding:?}, {orientation:?}, {viewport_mode:?}, {min_elevation:?}");
            assert!(z < 1.0, "{case}: ground {depth} px away is past the far plane at {}", state.view().far_z());
        }
    }
}

/// Padding that moves the sky's horizon far up the map keeps the far plane
/// past the ground the view shows: a 2000 by 500 view rolled 90 degrees
/// with 1000 px of left padding, with or without terrain.
#[test]
fn padding_far_up_the_map_keeps_the_far_plane() {
    let mut state = rolled(2000, 500, 30.0, 90.0, crate::geo::EdgeInsets::new(0.0, 1000.0, 0.0, 0.0));
    for min_elevation in [None, Some(0.0)] {
        state.set_terrain_min_elevation(min_elevation);
        let ground = visible_sea_level(&state);
        assert!(ground.len() > 1000, "{} points in view", ground.len());
        let (depth, z) = ground.iter().copied().fold((0.0, 0.0), |a, b| if b.0 > a.0 { b } else { a });
        let far = state.view().far_z();
        assert!(z < 1.0, "{min_elevation:?}: ground {depth} px away is past the far plane at {far}");
    }
}
