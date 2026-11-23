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
    let proj_matrix = state.view().projection_matrix_with(1, false);
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
