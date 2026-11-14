// Camera commands: when their ends report, fits, and the viewport and tile
// settings. They follow maplibre-native-ffi's ABI tests
// (tests/native/abi/map/camera.c, camera_transitions.c and options.c at
// eae96a42c) where those test the same rules.

mod support;

use std::time::Duration;

use mln_map::{
    AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, NorthOrientation, TileLodMode, TransitionOutcome,
};
use mln_runtime::{
    CameraFitArea, CameraFitOptions, CameraFitUpdate, ErrorKind, EventMask, TileOptions, ViewportOptions,
};
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds};
use support::{map, runtime, wait, wait_err};

fn animation(duration: Duration) -> AnimationOptions {
    AnimationOptions { duration: Some(duration), ..AnimationOptions::default() }
}

fn zoom_to(zoom: f64, animation: AnimationOptions) -> CameraUpdate {
    CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(zoom), ..Default::default() },
        animation,
        ..CameraUpdate::default()
    }
}

#[test]
fn an_end_reports_the_snapshot_that_shows_it_before_its_completion() {
    let runtime = runtime();
    let map = map(&runtime);

    // The end arrives before the command's completion.
    let jump = map.update_camera(zoom_to(3.0, animation(Duration::ZERO))).unwrap();
    let jumped = wait(Ok(jump.applied));
    assert!(jump.ended.is_complete());
    let end = wait(Ok(jump.ended));
    assert_eq!((end.outcome, end.generation), (TransitionOutcome::Completed, jumped.generation));
    assert_eq!(jumped.generation, map.snapshot().unwrap().generation);

    let ease = map.update_camera(zoom_to(9.0, animation(Duration::from_secs(60)))).unwrap();
    let cancelled = wait(map.cancel_camera_transitions());
    let end = wait(Ok(ease.ended));
    assert_eq!((end.outcome, end.generation), (TransitionOutcome::Cancelled, cancelled.generation));

    let ease = map.update_camera(zoom_to(9.0, animation(Duration::from_secs(60)))).unwrap();
    wait(map.release());
    let end = wait(Ok(ease.ended));
    assert_eq!((end.outcome, end.generation), (TransitionOutcome::Closed, 0));
    wait(runtime.release());
}

#[test]
fn a_fit_moves_the_camera_where_the_query_fits() {
    let runtime = runtime();
    let map = map(&runtime);
    let area = CameraFitArea::Bounds(LatLngBounds::hull(
        LatLng::new(35.0, -125.0).unwrap(),
        LatLng::new(39.0, -115.0).unwrap(),
    ));
    let fit = CameraFitOptions { bearing: Some(5.0), ..CameraFitOptions::default() };
    let camera = wait(map.camera_for(area.clone(), fit));
    assert!((camera.center.unwrap().longitude() + 120.0).abs() < 1e-9);
    assert_eq!(camera.bearing, Some(5.0));

    wait(
        map.fit_camera(area.clone(), CameraFitUpdate { fit, ..CameraFitUpdate::default() })
            .map(|command| command.applied),
    );
    let moved = wait(map.camera()).camera;
    assert!((moved.zoom - camera.zoom.unwrap()).abs() < 1e-9);
    assert!((moved.bearing - 5.0).abs() < 1e-9);

    let crowded =
        CameraFitOptions { padding: EdgeInsets { left: 128.0, right: 128.0, ..EdgeInsets::default() }, ..fit };
    let refused = CameraFitUpdate { fit: crowded, animation: animation(Duration::from_secs(1)), ..Default::default() };
    let refused = map.fit_camera(area, refused).unwrap();
    let error = wait_err(Ok(refused.applied));
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    assert!(error.message.contains("padding"), "{}", error.message);
    assert_eq!(wait(Ok(refused.ended)).outcome, TransitionOutcome::Cancelled);
    assert_eq!(wait(map.camera()).camera, moved);

    assert_eq!(ErrorKind::InvalidArgument, wait_err(map.camera_for(CameraFitArea::Coordinates(Vec::new()), fit)).kind);
    let empty =
        CameraFitArea::Geometry(geo_types::Geometry::GeometryCollection(geo_types::GeometryCollection(Vec::new())));
    assert_eq!(ErrorKind::InvalidArgument, wait_err(map.camera_for(empty, fit)).kind);
    wait(map.release());
    wait(runtime.release());
}

#[test]
fn the_visible_bounds_unwrap_across_the_antimeridian() {
    let runtime = runtime();
    let map = map(&runtime);
    // 256 pixels at zoom 1 span 90 degrees.
    let camera =
        CameraOptions { center: Some(LatLng::new(0.0, 170.0).unwrap()), zoom: Some(1.0), ..Default::default() };
    let bounds = wait(map.lat_lng_bounds_for_camera(camera));
    assert!((bounds.west() - 125.0).abs() < 1e-6, "{bounds:?}");
    assert!((bounds.east() - 215.0).abs() < 1e-6, "{bounds:?}");
    wait(map.release());
    wait(runtime.release());
}

#[test]
fn viewport_and_tile_options_keep_what_they_leave_out() {
    let runtime = runtime();
    let map = map(&runtime);
    wait(map.set_viewport_options(ViewportOptions {
        north_orientation: Some(NorthOrientation::Rightwards),
        ..ViewportOptions::default()
    }));
    wait(map.set_viewport_options(ViewportOptions {
        frustum_offset: Some(EdgeInsets { top: 4.0, ..EdgeInsets::default() }),
        ..ViewportOptions::default()
    }));
    let viewport = map.snapshot().unwrap().view.viewport;
    assert_eq!(viewport.north_orientation, NorthOrientation::Rightwards);
    assert_eq!(viewport.frustum_offset.top, 4.0);

    let set = wait(map.set_tile_options(TileOptions {
        prefetch_zoom_delta: Some(2),
        lod_mode: Some(TileLodMode::Distance),
        lod_pitch_threshold: Some(50.0),
        ..TileOptions::default()
    }));
    let tile = map.snapshot().unwrap().tile;
    assert_eq!((tile.prefetch_zoom_delta, tile.lod_mode), (2, TileLodMode::Distance));
    // Sending back the threshold that the snapshot reports changes nothing.
    let again = wait(map.set_tile_options(TileOptions {
        lod_pitch_threshold: Some(tile.lod_pitch_threshold),
        ..TileOptions::default()
    }));
    assert_eq!(set.generation, again.generation);
    wait(map.release());
    wait(runtime.release());
}

#[test]
fn an_event_mask_change_is_a_new_generation() {
    let runtime = runtime();
    let map = map(&runtime);
    let before = map.snapshot().unwrap().generation;
    let changed = wait(map.set_event_mask(EventMask::NONE));
    assert!(changed.generation > before);
    assert_eq!(changed.generation, wait(map.set_event_mask(EventMask::NONE)).generation);
    wait(map.release());
    wait(runtime.release());
}

#[test]
fn a_jumps_end_callback_runs_on_the_runtimes_thread() {
    let runtime = runtime();
    let map = map(&runtime);
    let (sender, ended) = std::sync::mpsc::channel();
    for _ in 0..20 {
        let sender = sender.clone();
        let jumped = map.update_camera_then(zoom_to(3.0, animation(Duration::ZERO)), move |end| {
            sender.send((end.map(|end| end.outcome), std::thread::current().id())).unwrap();
        });
        wait(jumped);
    }
    let here = std::thread::current().id();
    for _ in 0..20 {
        let (outcome, thread) = ended.recv().unwrap();
        assert_eq!(outcome, Ok(TransitionOutcome::Completed));
        assert_ne!(thread, here);
    }
    wait(map.release());
    wait(runtime.release());
}
