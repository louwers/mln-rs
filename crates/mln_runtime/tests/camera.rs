// Camera commands: when their ends report, fits, and the viewport and tile
// settings. They follow the C ABI suite (tools/c-abi-test/testdata/tests/
// native/abi/map/camera.c, camera_transitions.c and options.c) where it tests
// the same rules.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use mln_engine::camera::{
    AnimationOptions, CameraOptions, CameraUpdate, CameraUpdateMode, NorthOrientation, TransitionEnd, TransitionOutcome,
};
use mln_engine::geo::EdgeInsets;
use mln_engine::tile_store::TileLodMode;
use mln_runtime::{
    CameraFitArea, CameraFitOptions, CameraFitUpdate, ErrorKind, EventMask, TileOptions, ViewportOptions,
};
use mln_style::geo::{LatLng, LatLngBounds};

use crate::support::{map, runtime, wait, wait_err};

fn animation(duration: Duration) -> AnimationOptions {
    AnimationOptions { duration: Some(duration), ..AnimationOptions::default() }
}

/// An end callback that sends the end, and where it arrives.
fn end_channel() -> (impl FnOnce(Result<TransitionEnd, mln_runtime::Error>) + Send + 'static, Receiver<TransitionEnd>) {
    let (sender, receiver): (Sender<TransitionEnd>, _) = channel();
    (move |end: Result<TransitionEnd, mln_runtime::Error>| sender.send(end.expect("an end")).unwrap(), receiver)
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
    let (on_end, ended) = end_channel();
    let jumped = wait(map.update_camera_then(zoom_to(3.0, animation(Duration::ZERO)), on_end));
    let end = ended.try_recv().expect("the end, before the completion");
    assert_eq!((end.outcome, end.generation), (TransitionOutcome::Completed, jumped.generation));
    assert_eq!(jumped.generation, map.snapshot().unwrap().generation);

    let (on_end, ended) = end_channel();
    drop(map.update_camera_then(zoom_to(9.0, animation(Duration::from_secs(60))), on_end).unwrap());
    let cancelled = wait(map.cancel_camera_transitions());
    let end = ended.try_recv().expect("the end, before the cancel's completion");
    assert_eq!((end.outcome, end.generation), (TransitionOutcome::Cancelled, cancelled.generation));

    let (on_end, ended) = end_channel();
    drop(map.update_camera_then(zoom_to(9.0, animation(Duration::from_secs(60))), on_end).unwrap());
    wait(map.release());
    let end = ended.recv().unwrap();
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

    wait(map.fit_camera(area.clone(), CameraFitUpdate { fit, ..CameraFitUpdate::default() }));
    let moved = wait(map.camera()).camera;
    assert!((moved.zoom - camera.zoom.unwrap()).abs() < 1e-9);
    assert!((moved.bearing - 5.0).abs() < 1e-9);

    let crowded =
        CameraFitOptions { padding: EdgeInsets { left: 128.0, right: 128.0, ..EdgeInsets::default() }, ..fit };
    let refused = CameraFitUpdate { fit: crowded, animation: animation(Duration::from_secs(1)), ..Default::default() };
    let (on_end, ended) = end_channel();
    let error = wait_err(map.fit_camera_then(area, refused, on_end));
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    assert!(error.message.contains("padding"), "{}", error.message);
    assert_eq!(ended.try_recv().unwrap().outcome, TransitionOutcome::Cancelled);
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
    let (sender, ended) = channel();
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
