//! The camera over terrain on the globe, which follows the rules it follows
//! on Mercator, with heights along each point's vertical: it stands on the
//! ground at rest, a command ends on it, and the eye stays above it.

use std::sync::Arc;
use std::time::Duration;

use glam::DVec3;
use mln_map_types::Elevation;
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::geo::{LatLng, WrapMode};
use mln_util::queue::Queue;
use mln_util::time::Instant;

use super::ground::{Plateau, STYLE, Slope, assert_near, flat, ground, ground_event, pitchable, report, world};
use super::terrain_camera::{frames, start};
use crate::{AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, Map};

/// A 512 by 512 map of [`STYLE`] under `vertical-perspective`, at rest at
/// [`start`] at zoom 14 and `pitch`, with its frame clock an hour ahead so
/// that the test steps it, over `ground`.
fn globe_over(ground: &Arc<dyn Elevation>, pitch: f64, run_loop: &Queue<()>) -> (Map, Instant) {
    let style = STYLE.replace(r#""layers": []"#, r#""layers": [], "projection": {"type": "vertical-perspective"}"#);
    let mut map = pitchable(&style, run_loop);
    let clock = Instant::now() + Duration::from_secs(3600);
    map.set_frame_time(Some(clock));
    map.jump_to(&CameraOptions { center: Some(start()), zoom: Some(14.0), pitch: Some(pitch), ..Default::default() });
    report(&mut map, ground_event(ground));
    assert!(map.transform_state().view().is_globe());
    (map, clock)
}

fn animated(map: &mut Map, mode: CameraUpdateMode, camera: CameraOptions) {
    map.update_camera(&CameraUpdate {
        mode,
        camera,
        animation: AnimationOptions {
            duration: Some(Duration::from_secs(1)),
            easing: Some(DEFAULT_TRANSITION_EASE),
            ..Default::default()
        },
        ..Default::default()
    });
}

/// At rest on the globe the center stands on the ground under it, and the
/// view's planes fit the ground there: the center draws at the middle of
/// the view standing on that ground.
#[test]
fn a_camera_at_rest_on_the_globe_stands_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let (map, _) = globe_over(&flat(Some(1200.0), 1000.0), 60.0, &run_loop);
    assert_eq!((1200.0, Some(1000.0)), ground(&map));
    let center = map.transform_state().view().center(WrapMode::Wrapped);
    let pixel = map.pixel_for_lat_lng(&center);
    assert!((pixel.x - 256.0).hypot(pixel.y - 256.0) < 0.01, "the center draws at {pixel:?}");
}

/// A flight on the globe over ground that rises toward its end ends at the
/// center and zoom it asked for, standing on the ground there.
#[test]
fn a_flight_on_the_globe_ends_on_the_ground() {
    let run_loop = Queue::<()>::new();
    let slope: Arc<dyn Elevation> = Arc::new(Slope { x: world(&start()).x, height: 1000.0, rise: 500_000.0 });
    let (mut map, clock) = globe_over(&slope, 40.0, &run_loop);
    assert_near(1000.0, ground(&map).0, 1e-6, "the altitude at the start");
    let destination = LatLng::new(20.05, 10.2).unwrap();
    let camera = CameraOptions { center: Some(destination), zoom: Some(13.0), ..Default::default() };
    animated(&mut map, CameraUpdateMode::Fly, camera);
    frames(&mut map, clock, Duration::from_secs(1), |_, _| {});

    assert!(!map.snapshot().transition_in_progress);
    let camera = map.transform_state().camera_options(None);
    let center = camera.center.unwrap();
    assert_near(destination.latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(destination.longitude(), center.longitude(), 1e-9, "longitude");
    assert_eq!(Some(13.0), camera.zoom);
    let end = slope.at(center.wrapped()).unwrap();
    assert!(end > 1200.0, "{end}");
    assert_eq!(end, camera.center_altitude.unwrap());
}

/// A pitch command on the globe that would carry the eye into a plateau
/// behind the center keeps the eye, and the near plane, above it on every
/// frame.
#[test]
fn the_eye_stays_above_a_plateau_while_a_pitch_command_runs_on_the_globe() {
    let run_loop = Queue::<()>::new();
    // The plateau stands under the eye, south of the center, which stands
    // at sea level.
    let plateau = Plateau { y: world(&start()).y + 1e-6, height: 2500.0 };
    let surface = |point: DVec3| plateau.surface_at(point.truncate()).unwrap();
    let ground: Arc<dyn Elevation> = Arc::new(Plateau { ..plateau });
    let (mut map, clock) = globe_over(&ground, 0.0, &run_loop);
    animated(&mut map, CameraUpdateMode::Ease, CameraOptions { pitch: Some(80.0), ..Default::default() });
    let mut clearance = f64::INFINITY;
    frames(&mut map, clock, Duration::from_secs(1), |map, elapsed| {
        let view = map.transform_state().view();
        let eye = view.eye_position();
        assert!(eye.z >= surface(eye) - 0.01, "{elapsed:?}: the eye at {eye}");
        // Each raise turns the near plane down a little, so after GL JS's
        // three the near plane may stay a few millimeters short.
        for point in view.near_plane_points() {
            assert!(point.z >= surface(point) - 0.01, "{elapsed:?}: the near plane at {point}");
        }
        clearance = clearance.min(eye.z - surface(eye));
    });
    let camera = map.transform_state().camera_options(None);
    assert!(camera.pitch.unwrap() < 80.0 && camera.zoom.unwrap() < 14.0, "raised by pitch and zoom: {camera:?}");
    assert!(clearance < 100.0, "the eye came down to {clearance} m above the plateau");
    let center = map.transform_state().view().center(WrapMode::Unwrapped);
    assert_near(start().latitude(), center.latitude(), 1e-9, "latitude");
    assert_near(start().longitude(), center.longitude(), 1e-9, "longitude");
}
