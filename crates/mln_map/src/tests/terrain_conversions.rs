//! How the map converts between screen points and locations over the
//! terrain: a point unprojects onto the ground the renderer drew under it,
//! and a location projects at the height it was drawn at.

use std::sync::Arc;

use glam::DVec2;
use mln_map_types::{Elevation, View};
use mln_util::geo::{LatLng, ScreenCoordinate, WrapMode};
use mln_util::queue::Queue;

use super::ground::{STYLE, assert_near, ground_event, pitchable, report, world};
use crate::transform::TerrainGround;
use crate::{AnimationOptions, CameraOptions, Map, MapProjection};

/// One pixel at zoom 12, in Web Mercator at zoom 0.
const PIXEL: f64 = 1.0 / (512.0 * 4096.0);

/// The style of [`STYLE`] without its terrain.
const FLAT_STYLE: &str = r#"{
    "version": 8,
    "center": [10, 20],
    "zoom": 3,
    "sources": {"dem": {"type": "raster-dem", "tiles": ["dem/{z}/{x}/{y}.png"]}},
    "layers": []
}"#;

fn center() -> LatLng {
    LatLng::new(20.0, 10.0).unwrap()
}

/// A ridge 800 m high running east and west, 100 pixels at zoom 12 south of
/// [`center`] and 60 pixels wide on each side, on sea level. The renderer drew
/// it only south of the world's `drawn_from` (Web Mercator at zoom 0).
#[derive(Debug)]
struct Ridge {
    y: f64,
    drawn_from: f64,
}

impl Ridge {
    const PEAK: f64 = 800.0;
    const HALF_WIDTH: f64 = 60.0 * PIXEL;

    fn drawn_from(drawn_from: f64) -> Arc<dyn Elevation> {
        Arc::new(Ridge { y: world(&center()).y + 100.0 * PIXEL, drawn_from })
    }

    fn surface(&self, world: DVec2) -> Option<f64> {
        (world.y >= self.drawn_from).then(|| Self::PEAK * (1.0 - (world.y - self.y).abs() / Self::HALF_WIDTH).max(0.0))
    }
}

impl Elevation for Ridge {
    fn at(&self, location: LatLng) -> Option<f64> {
        self.surface(world(&location.wrapped()))
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(0.0)
    }

    fn surface_at(&self, world: DVec2) -> Option<f64> {
        self.surface(world)
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((0.0, Self::PEAK))
    }
}

/// A 512 by 512 map of `style` at rest at zoom 12 over [`center`], unless
/// `camera` gives a center, with the rest of `camera`, and `ground` drawn if
/// any.
fn map_over(style: &str, ground: Option<&Arc<dyn Elevation>>, camera: CameraOptions, run_loop: &Queue<()>) -> Map {
    let mut map = pitchable(style, run_loop);
    map.jump_to(&CameraOptions { center: camera.center.or(Some(center())), zoom: Some(12.0), ..camera });
    if let Some(ground) = ground {
        report(&mut map, ground_event(ground));
    }
    map
}

fn pitched(pitch: f64) -> CameraOptions {
    CameraOptions { pitch: Some(pitch), ..Default::default() }
}

/// `point` (y down) measured from the bottom edge of `view`.
fn flipped(view: &View, point: &ScreenCoordinate) -> ScreenCoordinate {
    ScreenCoordinate::new(point.x, f64::from(view.size().height) - point.y)
}

/// Pixels over the lower half of a 512 by 512 view.
fn lower_half() -> impl Iterator<Item = ScreenCoordinate> {
    (0..=8).flat_map(|row| {
        (0..=8).map(move |column| ScreenCoordinate::new(16.0 + 60.0 * f64::from(column), 270.0 + 28.0 * f64::from(row)))
    })
}

fn assert_same_pixel(expected: ScreenCoordinate, actual: ScreenCoordinate, tolerance: f64, what: &str) {
    assert!(
        (expected.x - actual.x).abs() <= tolerance && (expected.y - actual.y).abs() <= tolerance,
        "{what}: {actual:?}, not {expected:?}"
    );
}

/// A pixel unprojects to where its line of sight meets the ground the
/// renderer drew, and that location projects back to the pixel at the
/// height it was drawn at, whether on the ridge or on the sea level around
/// it. A pan by the map's own move solves on a plane, not on the ground
/// under the pointer, which would move the center by the ground's parallax.
#[test]
fn unproject_meets_the_terrain_and_project_returns_to_the_pixel() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(0.0);
    let mut map = map_over(STYLE, Some(&ridge), pitched(60.0), &run_loop);
    assert_eq!(0.0, map.transform_state().view().center_altitude());

    let pixels: Vec<_> = lower_half().collect();
    let mut on_the_ridge = 0;
    for pixel in &pixels {
        let location = map.lat_lng_for_pixel(pixel, WrapMode::Unwrapped).unwrap();
        let height = ridge.surface_at(world(&location)).unwrap();
        let back = map.pixel_for_lat_lng(&location);
        assert_same_pixel(*pixel, back, 0.01, "the round trip");
        if height > 100.0 {
            on_the_ridge += 1;
            // Sea level under the pixel lies beyond the ridge, elsewhere on
            // the screen.
            let view = map.transform_state().view();
            let sea_level = flipped(view, &view.lat_lng_to_screen_coordinate(&location));
            assert!((sea_level.y - pixel.y).abs() > 1.0, "{pixel:?} meets the ridge at {height} m");
        }
    }
    assert!(on_the_ridge > 9, "{on_the_ridge} pixels of the ridge");
    let each: Vec<_> = pixels.iter().map(|pixel| map.lat_lng_for_pixel(pixel, WrapMode::Wrapped).unwrap()).collect();
    assert_eq!(each, map.lat_lngs_for_pixels(&pixels, WrapMode::Wrapped).unwrap());

    // The pan's point lies on the ridge; the center at sea level, where it
    // stands, is the plane it pans on.
    let point = ScreenCoordinate::new(256.0, 330.0);
    let offset = ScreenCoordinate::new(0.0, 256.0 - point.y);
    let grabbed = ridge.surface_at(world(&map.lat_lng_for_pixel(&point, WrapMode::Unwrapped).unwrap())).unwrap();
    assert!(grabbed > 100.0, "{grabbed}");
    let view = map.transform_state().view().clone();
    let expected = view.screen_coordinate_to_lat_lng(&flipped(&view, &point), WrapMode::Unwrapped).unwrap();
    map.move_by(&offset, &AnimationOptions::default()).unwrap();
    let center = map.transform_state().view().center(WrapMode::Unwrapped);
    assert_near(expected.latitude(), center.latitude(), 1e-12, "latitude");
    assert_near(expected.longitude(), center.longitude(), 1e-12, "longitude");
}

/// Where the line of sight meets no ground the renderer drew, or before it
/// drew any, a pixel unprojects onto the plane at the center's altitude,
/// above the horizon as below it.
#[test]
fn unproject_into_the_sky_meets_the_centre_plane() {
    let run_loop = Queue::<()>::new();
    // Drawn only from the center south: the top of a pitched view looks
    // over ground that wasn't drawn.
    let ridge = Ridge::drawn_from(world(&center()).y);
    for (ground, pitch) in [(Some(&ridge), 60.0), (Some(&ridge), 85.0), (None, 60.0)] {
        let camera = CameraOptions { center_altitude: Some(1500.0), ..pitched(pitch) };
        let mut map = pitchable(STYLE, &run_loop);
        map.set_center_clamped_to_ground(false);
        map.jump_to(&CameraOptions { center: Some(center()), zoom: Some(12.0), ..camera });
        if let Some(ground) = ground {
            report(&mut map, ground_event(ground));
        }
        let view = map.transform_state().view().clone();
        assert_eq!(1500.0, view.center_altitude());

        for pixel in [ScreenCoordinate::new(40.0, 0.0), ScreenCoordinate::new(256.0, 30.0)] {
            let point = flipped(&view, &pixel);
            let expected = view.screen_coordinate_to_lat_lng_at(&point, 1500.0, WrapMode::Unwrapped).unwrap();
            let actual = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            assert_eq!(expected, actual, "{pixel:?} at pitch {pitch}");
            if pitch < 80.0 {
                let sea_level = view.screen_coordinate_to_lat_lng(&point, WrapMode::Unwrapped).unwrap();
                assert!((sea_level.latitude() - actual.latitude()).abs() > 1e-3, "{pixel:?} meets sea level");
            }
        }
        let nan = ScreenCoordinate::new(f64::NAN, 0.0);
        assert!(map.lat_lng_for_pixel(&nan, WrapMode::Wrapped).is_err());
    }
}

/// A projection made from the map converts on the ground the map's camera
/// stands on, as the map does; one made from a view converts on the flat
/// map, unless it is given the ground.
#[test]
fn a_projection_from_the_map_carries_its_ground() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(0.0);
    let map = map_over(STYLE, Some(&ridge), pitched(60.0), &run_loop);
    let ground = map.ground().expect("the ground");
    assert!(std::ptr::addr_eq(Arc::as_ptr(&ground), Arc::as_ptr(&ridge)));

    let view = map.snapshot().view;
    let from_map = MapProjection::new(&map);
    let flat = MapProjection::from_view(&view).unwrap();
    let grounded = MapProjection::from_view(&view).unwrap().with_terrain(map.terrain());
    let state_view = map.transform_state().view();
    let mut differ = 0;
    for pixel in lower_half() {
        let expected = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_eq!(expected, from_map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap());
        let on_ground = grounded.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_near(expected.latitude(), on_ground.latitude(), 1e-9, "latitude on the ground");
        assert_near(expected.longitude(), on_ground.longitude(), 1e-9, "longitude on the ground");

        let flat_location = flat.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        let sea_level =
            state_view.screen_coordinate_to_lat_lng(&flipped(state_view, &pixel), WrapMode::Unwrapped).unwrap();
        assert_near(sea_level.latitude(), flat_location.latitude(), 1e-9, "latitude at sea level");
        if (sea_level.latitude() - expected.latitude()).abs() > 1e-6 {
            differ += 1;
        }

        assert_eq!(map.pixel_for_lat_lng(&expected), from_map.pixel_for_lat_lng(&expected));
        assert_same_pixel(pixel, grounded.pixel_for_lat_lng(&expected), 0.01, "the ground's round trip");
    }
    assert!(differ > 9, "{differ} pixels meet the ridge");

    let flat_map = map_over(FLAT_STYLE, None, pitched(60.0), &run_loop);
    assert!(flat_map.ground().is_none());
}

/// Without terrain, the map and its projections convert on the flat map,
/// to the bit, at any pitch, bearing, padding, center altitude and copy of
/// the world.
#[test]
fn without_terrain_conversions_are_unchanged() {
    let run_loop = Queue::<()>::new();
    let cameras = [
        CameraOptions::default(),
        CameraOptions { pitch: Some(45.0), bearing: Some(30.0), ..Default::default() },
        CameraOptions {
            pitch: Some(60.0),
            bearing: Some(-120.0),
            padding: Some(mln_util::geo::EdgeInsets::new(100.0, 20.0, 0.0, 50.0)),
            ..Default::default()
        },
        CameraOptions { pitch: Some(30.0), center_altitude: Some(1200.0), ..Default::default() },
        CameraOptions { center: Some(LatLng::new(-10.0, 179.9).unwrap()), pitch: Some(20.0), ..Default::default() },
    ];
    for camera in cameras {
        let map = map_over(FLAT_STYLE, None, camera, &run_loop);
        assert!(map.ground().is_none());
        let view = map.transform_state().view();
        let from_map = MapProjection::new(&map);
        let from_view = MapProjection::from_view(&map.snapshot().view).unwrap();
        for pixel in lower_half().chain([ScreenCoordinate::new(0.0, 0.0), ScreenCoordinate::new(500.0, 120.0)]) {
            for wrap_mode in [WrapMode::Wrapped, WrapMode::Unwrapped] {
                let expected = view.screen_coordinate_to_lat_lng(&flipped(view, &pixel), wrap_mode).unwrap();
                assert_eq!(expected, map.lat_lng_for_pixel(&pixel, wrap_mode).unwrap(), "{camera:?}");
                assert_eq!(expected, from_map.lat_lng_for_pixel(&pixel, wrap_mode).unwrap(), "{camera:?}");
                let location = expected.wrapped();
                let expected = flipped(view, &view.lat_lng_to_screen_coordinate(&view.lat_lng_near_center(&location)));
                assert_eq!(expected, map.pixel_for_lat_lng(&location), "{camera:?}");
                assert_eq!(expected, from_map.pixel_for_lat_lng(&location), "{camera:?}");
                assert_same_pixel(expected, from_view.pixel_for_lat_lng(&location), 1e-6, "from the view");
            }
        }
    }
}

/// A projection rebuilt from a map's view and given its terrain clips as
/// the map does, to the terrain's lowest ground, so the far rows of a
/// camera at sea level over ground 1,000 m below it meet that ground.
#[test]
fn a_projection_from_a_view_sees_as_far_as_the_map() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(STYLE, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions {
        center: Some(center()),
        zoom: Some(12.0),
        center_altitude: Some(0.0),
        ..pitched(60.0)
    });
    report(&mut map, ground_event(&super::ground::flat(Some(-1000.0), -1000.0)));
    assert_eq!(Some(-1000.0), map.transform_state().view().terrain_min_elevation());

    let projection = MapProjection::from_view(&map.snapshot().view).unwrap().with_terrain(map.terrain());
    let mut sunken = 0;
    for row in 0..8 {
        let pixel = ScreenCoordinate::new(256.0, 5.0 * f64::from(row));
        let expected = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        let actual = projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_near(expected.latitude(), actual.latitude(), 1e-9, "latitude");
        assert_near(expected.longitude(), actual.longitude(), 1e-9, "longitude");
        let view = map.transform_state().view();
        let plane = view.screen_coordinate_to_lat_lng_at(&flipped(view, &pixel), 0.0, WrapMode::Unwrapped).unwrap();
        sunken += usize::from((plane.latitude() - expected.latitude()).abs() > 1e-6);
    }
    assert_eq!(8, sunken, "pixels whose line of sight meets the sunken ground");
}

/// While terrain applies but the renderer has drawn no ground yet, a
/// projection of the map, or of its view given its terrain, falls back as
/// the map does: a pixel unprojects onto the plane at the center's altitude.
#[test]
fn a_projection_before_any_ground_falls_back_as_the_map_does() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(STYLE, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions {
        center: Some(center()),
        zoom: Some(12.0),
        center_altitude: Some(1500.0),
        ..pitched(60.0)
    });
    assert!(map.ground().is_none());

    let from_view = MapProjection::from_view(&map.snapshot().view).unwrap().with_terrain(map.terrain());
    for projection in [MapProjection::new(&map), from_view] {
        for pixel in lower_half() {
            let expected = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            let actual = projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            assert_near(expected.latitude(), actual.latitude(), 1e-9, "latitude");
            assert_near(expected.longitude(), actual.longitude(), 1e-9, "longitude");
            let location = expected.wrapped();
            assert_same_pixel(map.pixel_for_lat_lng(&location), projection.pixel_for_lat_lng(&location), 1e-6, "back");
        }
    }
}

/// A projection given terrain again clips to the ground it is given last,
/// and given none, to the flat map's planes: as a projection given only
/// that terrain does.
#[test]
fn a_projection_given_terrain_again_clips_to_the_last() {
    let run_loop = Queue::<()>::new();
    let mut map = pitchable(STYLE, &run_loop);
    map.set_center_clamped_to_ground(false);
    map.jump_to(&CameraOptions {
        center: Some(center()),
        zoom: Some(12.0),
        center_altitude: Some(0.0),
        ..pitched(60.0)
    });
    report(&mut map, ground_event(&super::ground::flat(Some(-1000.0), -1000.0)));
    let view = map.snapshot().view;
    let level = Some(TerrainGround { elevation: Some(super::ground::flat(Some(0.0), 0.0)), clamped: false });
    let pairs = [
        (MapProjection::from_view(&view).unwrap().with_terrain(map.terrain()), map.terrain()),
        (
            MapProjection::from_view(&view).unwrap().with_terrain(level.clone()).with_terrain(map.terrain()),
            map.terrain(),
        ),
        (MapProjection::from_view(&view).unwrap().with_terrain(map.terrain()).with_terrain(None), None),
        (MapProjection::from_view(&view).unwrap().with_terrain(map.terrain()).with_terrain(level.clone()), level),
    ];
    for (index, (projection, terrain)) in pairs.into_iter().enumerate() {
        let once = MapProjection::from_view(&view).unwrap().with_terrain(terrain);
        for row in 0..8 {
            let pixel = ScreenCoordinate::new(256.0, 5.0 * f64::from(row));
            let expected = once.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            let actual = projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            assert_eq!(expected, actual, "{index}: row {row}");
        }
    }
}
