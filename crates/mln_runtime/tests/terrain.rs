// What a host reads of a map over terrain: its projection, which converts on
// the ground the renderer drew and reads its heights, and whether its center
// stands on that ground.

use mln_map::{CameraOptions, CameraUpdate, MapProjection};
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};

use crate::support::{level_terrain_map, runtime, serve_level_dem, wait};

fn near(a: LatLng, b: LatLng, tolerance: f64) -> bool {
    (a.latitude() - b.latitude()).abs() <= tolerance && (a.longitude() - b.longitude()).abs() <= tolerance
}

/// The map's projection converts on the ground the renderer last drew,
/// 1000 m up here, and reads its height there; it reads none before the
/// first still draws the ground, off the drawn tiles, and on the globe,
/// where terrain doesn't draw.
#[test]
fn a_maps_projection_meets_and_reads_the_drawn_ground() {
    let runtime = runtime();
    serve_level_dem(&runtime);
    let (map, session) = level_terrain_map(&runtime);
    let center = LatLng::new(10.0, 20.0).unwrap();
    assert_eq!(None, map.projection().unwrap().terrain_elevation(&center), "before the first still");

    wait(map.request_still_image());
    let projection = map.projection().unwrap();
    let elevation = projection.terrain_elevation(&center).expect("the center was drawn");
    assert!((elevation - 1000.0).abs() <= 0.1, "{elevation}");
    let far = LatLng::new(-60.0, -100.0).unwrap();
    assert_eq!(None, projection.terrain_elevation(&far), "off the drawn tiles");

    // A pixel below the center meets the ground 1000 m up, where the flat
    // map's projection meets sea level.
    let snapshot = map.snapshot().unwrap();
    let state = snapshot.view.to_state().unwrap();
    let view = state.view();
    let pixel = ScreenCoordinate::new(128.0, 200.0);
    let up = ScreenCoordinate::new(pixel.x, f64::from(view.size().height) - pixel.y);
    let ground = view.screen_coordinate_to_lat_lng_at(&up, 1000.0, WrapMode::Unwrapped).unwrap();
    let on_ground = projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
    assert!(near(on_ground, ground, 1e-6), "{on_ground:?}, not {ground:?}");
    let flat = MapProjection::from_view(&snapshot.view).unwrap();
    let at_sea_level = flat.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
    assert!(!near(at_sea_level, ground, 1e-3), "{at_sea_level:?} {ground:?}");
    let back = projection.pixel_for_lat_lng(&on_ground);
    assert!((back.x - pixel.x).abs() < 0.01 && (back.y - pixel.y).abs() < 0.01, "{back:?}");

    // The globe draws below zoom 12.
    wait(map.set_style_root_property("projection", r#"{"type":"globe"}"#));
    let camera = CameraOptions { zoom: Some(3.0), ..Default::default() };
    wait(map.update_camera(CameraUpdate { camera, ..Default::default() }));
    wait(map.request_still_image());
    assert!(map.snapshot().unwrap().globe_rendering);
    assert_eq!(None, map.projection().unwrap().terrain_elevation(&center), "on the globe");

    drop(session);
    wait(map.release());
    assert!(map.projection().is_err(), "after release");
}

/// The clamp setter changes the snapshot's setting, and turning it on
/// stands the center on the ground.
#[test]
fn the_center_clamps_to_the_ground_when_the_setter_says() {
    let runtime = runtime();
    serve_level_dem(&runtime);
    let (map, session) = level_terrain_map(&runtime);
    wait(map.request_still_image());
    let snapshot = map.snapshot().unwrap();
    assert!(!snapshot.center_clamped_to_ground);
    assert_eq!(0.0, snapshot.view.camera.center_altitude);

    let applied = wait(map.set_center_clamped_to_ground(true));
    let snapshot = map.snapshot().unwrap();
    assert!(applied.generation > 0 && snapshot.generation >= applied.generation);
    assert!(snapshot.center_clamped_to_ground);
    let altitude = snapshot.view.camera.center_altitude;
    assert!((altitude - 1000.0).abs() <= 0.1, "{altitude}");

    wait(map.set_center_clamped_to_ground(false));
    assert!(!map.snapshot().unwrap().center_clamped_to_ground);

    drop(session);
    wait(map.release());
}
