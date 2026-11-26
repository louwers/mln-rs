// A runtime map as the camera that mln_gestures' recognizer moves: its reads
// come from the published snapshot, and its commands go out without waiting.

use mln_gestures::{GestureCamera, GestureRecognizer, Input, Modes, PointerKind};
use mln_map::{CameraOptions, CameraUpdate};
use mln_style::geo::{LatLng, WrapMode};
use mln_util::geo::ScreenCoordinate;

use crate::support::{level_terrain_map, map, runtime, serve_level_dem, wait};

fn at(x: f64, y: f64) -> ScreenCoordinate {
    ScreenCoordinate::new(x, y)
}

#[test]
fn the_recognizer_reads_and_moves_a_runtime_map() {
    let runtime = runtime();
    let map = map(&runtime);
    let center = LatLng::new(10.0, 20.0).unwrap();
    let update = CameraUpdate {
        camera: CameraOptions { center: Some(center), zoom: Some(4.0), ..Default::default() },
        ..Default::default()
    };
    wait(map.update_camera(update));

    // The reads are the snapshot's: the map is 256 pixels square.
    assert_eq!(GestureCamera::viewport(&map), at(256.0, 256.0));
    assert_eq!(GestureCamera::camera(&map).zoom, Some(4.0));
    let (min, max) = GestureCamera::zoom_range(&map);
    assert!(min <= 4.0 && 4.0 <= max, "{min} {max}");
    let under = GestureCamera::lat_lng_for_pixel(&map, at(128.0, 128.0)).unwrap();
    assert!((under.latitude() - 10.0).abs() < 1e-6 && (under.longitude() - 20.0).abs() < 1e-6, "{under:?}");

    // A drag to the right moves the map's content right: the center goes west.
    let mut gestures = GestureRecognizer::new(map.clone());
    let down =
        Input::PointerDown { id: 1, kind: PointerKind::Mouse, position: at(100.0, 128.0), modes: Modes::default() };
    gestures.handle(down, 0.0);
    for step in 1..=10 {
        let position = at(100.0 + 10.0 * f64::from(step), 128.0);
        gestures.handle(Input::PointerMove { id: 1, position }, 16.0 * f64::from(step));
    }
    // Held still past the release window, so no glide follows.
    gestures.handle(Input::PointerUp { id: 1, position: at(200.0, 128.0) }, 1000.0);
    let moved = wait(map.camera()).camera;
    assert!(moved.center.longitude() < 20.0, "{:?}", moved.center);
    assert!((moved.zoom - 4.0).abs() < 1e-9);

    wait(map.release());
    wait(runtime.release());
}

/// A box zoom over terrain centers on the ground under the box's middle,
/// which the map publishes beside its snapshot for the recognizer's reads:
/// here ground 1000 m high under a center that the terrain doesn't clamp,
/// at sea level. The map's projection converts on that ground too.
#[test]
fn a_box_zoom_over_terrain_centers_on_the_boxed_ground() {
    let runtime = runtime();
    serve_level_dem(&runtime);
    let (map, session) = level_terrain_map(&runtime);
    // The still draws the terrain, and the map publishes its ground.
    wait(map.request_still_image());
    let snapshot = map.snapshot().unwrap();
    assert_eq!(0.0, snapshot.view.camera.center_altitude);

    // The box's middle, below the center, meets the ground 1000 m up.
    let middle = at(128.0, 200.0);
    let state = snapshot.view.to_state().unwrap();
    let view = state.view();
    let up = ScreenCoordinate::new(middle.x, f64::from(view.size().height) - middle.y);
    let ground = view.screen_coordinate_to_lat_lng_at(&up, 1000.0, WrapMode::Unwrapped).unwrap();
    let sea_level = view.screen_coordinate_to_lat_lng_at(&up, 0.0, WrapMode::Unwrapped).unwrap();
    let near = |a: LatLng, b: LatLng, tolerance: f64| {
        (a.latitude() - b.latitude()).abs() <= tolerance && (a.longitude() - b.longitude()).abs() <= tolerance
    };
    assert!(!near(ground, sea_level, 1e-3), "{ground:?} {sea_level:?}");
    let under = GestureCamera::lat_lng_for_pixel(&map, middle).unwrap();
    assert!(near(under, ground, 1e-6), "{under:?}, not {ground:?}");
    let projected = map.projection(None).unwrap().lat_lng_for_pixel(&middle, WrapMode::Unwrapped).unwrap();
    assert_eq!(under, projected);

    let mut gestures = GestureRecognizer::new(map.clone());
    let modes = Modes { box_zoom: true, ..Modes::default() };
    gestures.handle(Input::PointerDown { id: 1, kind: PointerKind::Mouse, position: at(78.0, 150.0), modes }, 0.0);
    gestures.handle(Input::PointerMove { id: 1, position: at(178.0, 250.0) }, 16.0);
    gestures.handle(Input::PointerUp { id: 1, position: at(178.0, 250.0) }, 32.0);
    // A still map's ease ends with the next still.
    wait(map.request_still_image());
    let boxed = wait(map.camera()).camera;
    assert!(near(boxed.center, ground, 1e-6), "{:?}, not {ground:?}", boxed.center);
    assert!((boxed.zoom - (12.0 + 2.56f64.log2())).abs() < 1e-9, "{}", boxed.zoom);

    drop(session);
    wait(map.release());
}
