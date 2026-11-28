//! Where a map's projection puts places when its camera is at or past the
//! antimeridian.

use std::time::Duration;

use mln_engine::camera::{AnimationOptions, BoundOptions, CameraDelta, CameraOptions, CameraUpdate, GesturePhase};
use mln_engine::geo::ScreenCoordinate;
use mln_engine::map::MapMode;
use mln_runtime::{Map, Runtime};
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::size::Size;

use crate::support::still::assert_near;
use crate::support::{map_options, result_of, runtime, wait};

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

/// A map `side` pixels square in `mode`, with no render session, so an
/// animation stays where it starts.
fn square_map(runtime: &Runtime, side: u32, mode: MapMode) -> Map {
    let mut options = map_options();
    options.map.size = Size::new(side, side);
    options.map.map_mode = mode;
    let (map, created) = runtime.create_map(options).unwrap();
    result_of(created).unwrap();
    map
}

fn jump(map: &Map, camera: CameraOptions) {
    wait(map.update_camera(CameraUpdate { camera, ..CameraUpdate::default() }));
}

fn center_longitude(map: &Map) -> f64 {
    CameraOptions::from(wait(map.camera()).camera).center.unwrap().longitude()
}

/// A zoom around a point keeps the place under it. Across the antimeridian
/// the center comes back wrapped, as GL JS's `setLocationAtPoint` leaves it,
/// or the projection puts every place a world away.
#[test]
fn pixel_for_lat_lng_after_zoom_around_a_point_across_the_antimeridian() {
    let runtime = runtime();
    let map = square_map(&runtime, 256, MapMode::Static);
    jump(&map, CameraOptions { center: Some(ll(0.0, 179.9)), zoom: Some(5.0), ..Default::default() });

    let anchor = ScreenCoordinate::new(230.0, 128.0);
    let place = map.projection(None).unwrap().lat_lng_for_pixel(&anchor, WrapMode::Wrapped).unwrap();
    jump(&map, CameraOptions { zoom: Some(6.0), anchor: Some(anchor), ..Default::default() });

    assert!(center_longitude(&map).abs() <= 180.0);
    let point = map.projection(None).unwrap().pixel_for_lat_lng(&place);
    assert_near(anchor.x, point.x, 1e-6);
    assert_near(anchor.y, point.y, 1e-6);
}

/// A center past the antimeridian, which the camera's bounds allow, puts
/// each place at its copy nearest the center.
#[test]
fn pixel_for_lat_lng_with_the_center_past_the_antimeridian() {
    let runtime = runtime();
    let map = square_map(&runtime, 512, MapMode::Static);
    let bounds = LatLngBounds::hull(ll(-10.0, 170.0), ll(10.0, 190.0));
    wait(map.set_bounds(BoundOptions { bounds: Some(bounds), ..Default::default() }));
    jump(&map, CameraOptions { center: Some(ll(0.0, 181.0)), zoom: Some(4.0), ..Default::default() });
    assert_near(181.0, center_longitude(&map), 1e-9);

    // A degree is 512 * 2^4 / 360 pixels at zoom 4.
    let degree = 512.0 * 16.0 / 360.0;
    let projection = map.projection(None).unwrap();
    for (longitude, x) in [(179.0, 256.0 - 2.0 * degree), (-179.5, 256.0 - 0.5 * degree)] {
        assert_near(x, projection.pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
    }
}

/// A fling across the antimeridian starts a world away, and the projection
/// keeps each place at its copy nearest the center meanwhile
/// (maplibre/maplibre-native#3580: iOS ends a pan with a move that has a
/// duration while the gesture is still in progress, and annotation views
/// placed through `pixelForLatLng` vanished until the center crossed).
#[test]
fn pixel_for_lat_lng_during_a_fling_across_the_antimeridian() {
    let runtime = runtime();
    let map = square_map(&runtime, 512, MapMode::Continuous);
    jump(&map, CameraOptions { center: Some(ll(0.0, 179.0)), zoom: Some(4.0), ..Default::default() });

    let degree = 512.0 * 16.0 / 360.0;
    wait(map.apply_camera_delta(CameraDelta {
        offset: Some(ScreenCoordinate::new(-2.0 * degree, 0.0)),
        animation: AnimationOptions { duration: Some(Duration::from_secs(10)), ..AnimationOptions::default() },
        phase: GesturePhase::Begin,
        ..CameraDelta::default()
    }));
    let center = center_longitude(&map);
    assert!(center < -180.0, "{center}");

    let projection = map.projection(None).unwrap();
    for (longitude, nearest) in [(179.5, -180.5), (-179.5, -179.5)] {
        let x = 256.0 + (nearest - center) * degree;
        assert_near(x, projection.pixel_for_lat_lng(&ll(0.0, longitude)).x, 1e-6);
    }
}
