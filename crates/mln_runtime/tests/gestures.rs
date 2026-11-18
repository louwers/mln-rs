// A runtime map as the camera that mln_gestures' recognizer moves: its reads
// come from the published snapshot, and its commands go out without waiting.

use mln_gestures::{GestureCamera, GestureRecognizer, Input, Modes, PointerKind};
use mln_map::{CameraOptions, CameraUpdate};
use mln_util::geo::{LatLng, ScreenCoordinate};

use crate::support::{map, runtime, wait};

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
