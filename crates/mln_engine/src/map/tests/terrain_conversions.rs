//! How the map converts between screen points and locations over the
//! terrain, and fits cameras there: a pixel meets the ground the renderer
//! drew under it, a location projects at the height it was drawn at, and
//! where neither is drawn the plane through the center stands in for it.

use std::sync::Arc;

use glam::DVec2;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};

use super::ground::{STYLE, assert_near, flat, ground_event, pitchable, report, world};
use crate::camera::{CameraOptions, Elevation, MapProjection, View};
use crate::exec::queue::Queue;
use crate::geo::world::WorldMapping;
use crate::geo::{EdgeInsets, ScreenCoordinate};
use crate::map::Map;

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

/// A 512 by 512 map of `style` at zoom 12 over [`center`], unless `camera`
/// gives a center, with the rest of `camera`, its center clamped to the
/// ground if `clamped`, and `ground` drawn if any.
fn map_over(
    style: &str,
    ground: Option<&Arc<dyn Elevation>>,
    clamped: bool,
    camera: CameraOptions,
    run_loop: &Queue<()>,
) -> Map {
    let mut map = pitchable(style, run_loop);
    map.set_center_clamped_to_ground(clamped);
    map.jump_to(&CameraOptions {
        center: camera.center.or(Some(center())),
        zoom: camera.zoom.or(Some(12.0)),
        ..camera
    });
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

/// The location under `pixel` (y down) of `view` on the plane `height` meters
/// above sea level.
fn on_plane(view: &View, pixel: &ScreenCoordinate, height: f64) -> LatLng {
    view.screen_coordinate_to_lat_lng_at(&flipped(view, pixel), height, WrapMode::Unwrapped).unwrap()
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

fn assert_same_place(expected: LatLng, actual: LatLng, tolerance: f64, what: &str) {
    assert_near(expected.latitude(), actual.latitude(), tolerance, &format!("{what}: latitude"));
    assert_near(expected.longitude(), actual.longitude(), tolerance, &format!("{what}: longitude"));
}

/// A pixel converts to where its line of sight meets the ground the
/// renderer drew, on the ridge as on the sea level around it, and that
/// location converts back to the pixel at the height it was drawn at. On
/// the ridge, sea level under the pixel lies elsewhere on the screen.
#[test]
fn a_pixel_meets_the_drawn_ground_and_its_location_stands_on_it() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(0.0);
    let map = map_over(STYLE, Some(&ridge), true, pitched(60.0), &run_loop);
    let view = map.transform_state().view();
    assert_eq!(0.0, view.center_altitude());

    let pixels: Vec<_> = lower_half().collect();
    let mut on_the_ridge = 0;
    for pixel in &pixels {
        let location = map.lat_lng_for_pixel(pixel, WrapMode::Unwrapped).unwrap();
        let height = ridge.surface_at(world(&location)).unwrap();
        assert_same_pixel(*pixel, map.pixel_for_lat_lng(&location), 0.01, "the round trip");
        if height > 100.0 {
            on_the_ridge += 1;
            let sea_level = flipped(view, &view.lat_lng_to_screen_coordinate(&location));
            assert!((sea_level.y - pixel.y).abs() > 1.0, "{pixel:?} meets the ridge at {height} m");
        }
    }
    assert!(on_the_ridge > 9, "{on_the_ridge} pixels of the ridge");
    let each: Vec<_> = pixels.iter().map(|pixel| map.lat_lng_for_pixel(pixel, WrapMode::Wrapped).unwrap()).collect();
    assert_eq!(each, map.lat_lngs_for_pixels(&pixels, WrapMode::Wrapped).unwrap());
}

/// Where the line of sight meets no ground the renderer drew, as past the
/// ground drawn and in the sky, a pixel converts on the plane at the
/// center's altitude, and the location converts back there: the center's
/// pixel converts to the center, whatever ground was drawn.
#[test]
fn where_no_ground_was_drawn_the_plane_through_the_center_stands_in() {
    let run_loop = Queue::<()>::new();
    // Drawn only from the center south: the top of a pitched view looks
    // over ground that wasn't drawn.
    let ridge = Ridge::drawn_from(world(&center()).y);
    for pitch in [60.0, 85.0] {
        let camera = CameraOptions { center_altitude: Some(1500.0), ..pitched(pitch) };
        let map = map_over(STYLE, Some(&ridge), false, camera, &run_loop);
        let view = map.transform_state().view();
        assert_eq!(1500.0, view.center_altitude());

        for pixel in [ScreenCoordinate::new(40.0, 0.0), ScreenCoordinate::new(256.0, 30.0)] {
            let actual = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            assert_eq!(on_plane(view, &pixel, 1500.0), actual, "{pixel:?} at pitch {pitch}");
            // Above the plane's horizon, at pitch 85, the line of sight meets
            // it nowhere.
            if pitch < 80.0 {
                assert_same_pixel(pixel, map.pixel_for_lat_lng(&actual), 1e-6, "back on the plane");
                let sea_level = on_plane(view, &pixel, 0.0);
                assert!((sea_level.latitude() - actual.latitude()).abs() > 1e-3, "{pixel:?} meets sea level");
            }
        }
        let middle = ScreenCoordinate::new(256.0, 256.0);
        assert_same_place(center(), map.lat_lng_for_pixel(&middle, WrapMode::Unwrapped).unwrap(), 1e-9, "the center");
        let nan = ScreenCoordinate::new(f64::NAN, 0.0);
        assert!(map.lat_lng_for_pixel(&nan, WrapMode::Wrapped).is_err());
    }
}

/// Until the renderer draws the terrain, the map draws flat, and converts
/// there, at sea level, as without terrain: here under a center 1500 m up.
#[test]
fn before_the_terrain_draws_the_map_converts_on_the_flat_map() {
    let run_loop = Queue::<()>::new();
    let camera = CameraOptions { center_altitude: Some(1500.0), ..pitched(60.0) };
    let map = map_over(STYLE, None, false, camera, &run_loop);
    assert!(map.ground().is_none());
    let view = map.transform_state().view();
    for pixel in lower_half() {
        let location = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_eq!(on_plane(view, &pixel, 0.0), location);
        assert_eq!(MapProjection::new(&map).lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap(), location);
    }
}

/// A projection made from the map converts as the map does, on its ground;
/// one made from a view converts on the flat map, and on the map's ground
/// once given it, to within rounding.
#[test]
fn a_projection_converts_on_the_ground_it_is_given() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(0.0);
    let map = map_over(STYLE, Some(&ridge), true, pitched(60.0), &run_loop);
    assert!(std::ptr::addr_eq(Arc::as_ptr(&map.ground().unwrap()), Arc::as_ptr(&ridge)));

    let view = map.snapshot().view;
    let from_map = MapProjection::new(&map);
    let flat = MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap();
    let grounded = MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap().with_ground(map.ground());
    let state_view = map.transform_state().view();
    let mut on_the_ridge = 0;
    for pixel in lower_half() {
        let expected = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_eq!(expected, from_map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap());
        assert_eq!(map.pixel_for_lat_lng(&expected), from_map.pixel_for_lat_lng(&expected));
        let on_ground = grounded.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_same_place(expected, on_ground, 1e-9, "on the ground");
        assert_same_pixel(pixel, grounded.pixel_for_lat_lng(&expected), 0.01, "the ground's round trip");

        let sea_level = on_plane(state_view, &pixel, 0.0);
        assert_same_place(sea_level, flat.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap(), 1e-9, "flat");
        on_the_ridge += usize::from((sea_level.latitude() - expected.latitude()).abs() > 1e-6);
    }
    assert!(on_the_ridge > 9, "{on_the_ridge} pixels meet the ridge");
}

/// A projection reads the height of its ground where the ground was drawn,
/// and none where it wasn't, or on the flat map.
#[test]
fn a_projection_reads_the_height_of_its_ground() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(world(&center()).y);
    let map = map_over(STYLE, Some(&ridge), true, pitched(60.0), &run_loop);
    let from_map = MapProjection::new(&map);
    let flat = MapProjection::from_view(&map.snapshot().view, &WorldMapping::Mercator).unwrap();

    let at = |pixels_south: f64| {
        let point = world(&center()) + DVec2::new(0.0, pixels_south * PIXEL);
        let point = geometry::Point::new(point.x, point.y);
        crate::geo::projection::Projection::unproject(
            &point,
            1.0 / mln_style::constants::TILE_SIZE_D,
            WrapMode::Wrapped,
        )
        .unwrap()
    };
    assert_near(Ridge::PEAK, from_map.elevation(&at(100.0)).unwrap(), 1e-6, "the peak");
    assert_near(Ridge::PEAK / 2.0, from_map.elevation(&at(130.0)).unwrap(), 1e-6, "the slope");
    assert_eq!(from_map.elevation(&at(-10.0)), None, "north of the drawn ground");
    assert_eq!(flat.elevation(&at(100.0)), None, "the flat map");
}

/// Ground the camera stands on under its center, at 500 m, that the
/// renderer didn't draw, as when the camera is below every tile's range.
#[derive(Debug)]
struct UndrawnUnderCenter;

impl Elevation for UndrawnUnderCenter {
    fn at(&self, _: LatLng) -> Option<f64> {
        Some(500.0)
    }

    fn min_at(&self, _: LatLng) -> Option<f64> {
        Some(500.0)
    }

    fn surface_at(&self, _: DVec2) -> Option<f64> {
        None
    }

    fn range(&self) -> Option<(f64, f64)> {
        Some((500.0, 500.0))
    }
}

/// A projection reads no height from ground the camera stands on but the
/// renderer didn't draw.
#[test]
fn a_projection_reads_no_height_of_ground_not_drawn() {
    let run_loop = Queue::<()>::new();
    let ground: Arc<dyn Elevation> = Arc::new(UndrawnUnderCenter);
    let map = map_over(STYLE, Some(&ground), true, pitched(60.0), &run_loop);
    assert_eq!(MapProjection::new(&map).elevation(&center()), None);
}

/// A projection given the map's ground fits its view's planes to it as the
/// map does, so the far rows of a camera at sea level over ground 1000 m
/// below it meet that ground, past the flat map's far plane; and a camera
/// it moves fits them again, to convert as the map moved there does.
#[test]
fn a_projection_sees_as_far_as_its_map() {
    let run_loop = Queue::<()>::new();
    let camera = CameraOptions { center_altitude: Some(0.0), ..pitched(60.0) };
    let sunken = flat(Some(-1000.0), -1000.0);
    let mut map = map_over(STYLE, Some(&sunken), false, camera, &run_loop);
    assert_eq!(Some(-1000.0), map.transform_state().view().terrain_min_elevation());

    let mut projection =
        MapProjection::from_view(&map.snapshot().view, &WorldMapping::Mercator).unwrap().with_ground(map.ground());
    let rows = || (0..8).map(|row| ScreenCoordinate::new(256.0, 5.0 * f64::from(row)));
    let check = |map: &Map, projection: &MapProjection, on_ground: bool| {
        for pixel in rows() {
            let expected = map.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            let actual = projection.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
            assert_same_place(expected, actual, 1e-9, &format!("{pixel:?}"));
            if on_ground {
                let view = map.transform_state().view();
                assert_same_place(on_plane(view, &pixel, -1000.0), actual, 1e-9, &format!("{pixel:?} on the ground"));
            }
        }
    };
    check(&map, &projection, true);

    let moved = CameraOptions { center: Some(LatLng::new(20.01, 10.01).unwrap()), ..pitched(70.0) };
    map.jump_to(&moved);
    projection.set_camera(&moved);
    check(&map, &projection, false);
}

/// A projection on the map's ground moves its camera around an anchor as
/// the map's `jump_to` does: the ground on the ridge under the anchor stays
/// under it through a zoom and a turn, where solving on sea level would
/// slide it away.
#[test]
fn a_projection_moves_around_an_anchor_on_its_ground() {
    let run_loop = Queue::<()>::new();
    let ridge = Ridge::drawn_from(0.0);
    let camera = CameraOptions { center_altitude: Some(0.0), ..pitched(60.0) };
    let mut map = map_over(STYLE, Some(&ridge), false, camera, &run_loop);
    let mut projection =
        MapProjection::from_view(&map.snapshot().view, &WorldMapping::Mercator).unwrap().with_ground(map.ground());

    let anchor = lower_half()
        .find(|pixel| {
            let location = map.lat_lng_for_pixel(pixel, WrapMode::Unwrapped).unwrap();
            ridge.surface_at(world(&location)).unwrap() > 400.0 && (pixel.x - 256.0).abs() > 100.0
        })
        .expect("a pixel high on the ridge, off the center");
    let grabbed = projection.lat_lng_for_pixel(&anchor, WrapMode::Unwrapped).unwrap();
    let moved = CameraOptions { zoom: Some(12.7), bearing: Some(15.0), anchor: Some(anchor), ..Default::default() };
    map.jump_to(&moved);
    projection.set_camera(&moved);

    let (expected, actual) = (map.transform_state().view().camera(), projection.camera());
    assert_same_place(expected.center, actual.center, 1e-9, "the center");
    assert_near(expected.zoom, actual.zoom, 1e-9, "the zoom");
    assert_near(expected.bearing, actual.bearing, 1e-9, "the bearing");
    assert_near(expected.center_altitude, actual.center_altitude, 1e-6, "the altitude");
    assert_same_pixel(anchor, projection.pixel_for_lat_lng(&grabbed), 0.01, "the grabbed ground");
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
            padding: Some(EdgeInsets::new(100.0, 20.0, 0.0, 50.0)),
            ..Default::default()
        },
        CameraOptions { pitch: Some(30.0), center_altitude: Some(1200.0), ..Default::default() },
        CameraOptions { center: Some(LatLng::new(-10.0, 179.9).unwrap()), pitch: Some(20.0), ..Default::default() },
    ];
    for camera in cameras {
        let map = map_over(FLAT_STYLE, None, true, camera, &run_loop);
        assert!(map.ground().is_none());
        let view = map.transform_state().view();
        let from_map = MapProjection::new(&map);
        for pixel in lower_half().chain([ScreenCoordinate::new(0.0, 0.0), ScreenCoordinate::new(500.0, 120.0)]) {
            for wrap_mode in [WrapMode::Wrapped, WrapMode::Unwrapped] {
                let expected = view.screen_coordinate_to_lat_lng(&flipped(view, &pixel), wrap_mode).unwrap();
                assert_eq!(expected, map.lat_lng_for_pixel(&pixel, wrap_mode).unwrap(), "{camera:?}");
                assert_eq!(expected, from_map.lat_lng_for_pixel(&pixel, wrap_mode).unwrap(), "{camera:?}");
                let location = expected.wrapped();
                let expected = flipped(view, &view.lat_lng_to_screen_coordinate(&view.lat_lng_near_center(&location)));
                assert_eq!(expected, map.pixel_for_lat_lng(&location), "{camera:?}");
                assert_eq!(expected, from_map.pixel_for_lat_lng(&location), "{camera:?}");
            }
        }
    }
}

/// A globe's view converts on the ground it is given, as on Mercator: a
/// pixel meets the ground 1000 m up, away from where the sea-level sphere
/// meets its line of sight, and that location converts back to the pixel.
#[test]
fn a_globe_view_converts_on_its_ground() {
    let run_loop = Queue::<()>::new();
    let globe_style = r#"{"version": 8, "center": [10, 20], "zoom": 3, "sources": {}, "layers": [],
        "projection": {"type": "globe"}}"#;
    let map = map_over(globe_style, None, true, CameraOptions { zoom: Some(3.0), ..pitched(30.0) }, &run_loop);
    let view = map.snapshot().view;
    assert!(map.snapshot().globe_rendering);

    let flat_globe = MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap();
    let globe =
        MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap().with_ground(Some(flat(Some(1000.0), 1000.0)));
    for pixel in lower_half() {
        let on_ground = globe.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        let at_sea_level = flat_globe.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        let moved =
            (on_ground.latitude() - at_sea_level.latitude()).hypot(on_ground.longitude() - at_sea_level.longitude());
        assert!(moved > 1e-4, "{pixel:?} meets the ground {moved} degrees from sea level");
        assert_same_pixel(pixel, globe.pixel_for_lat_lng(&on_ground), 0.01, "back on the ground");
    }
}

/// On the globe, the polar caps past the Mercator range draw at sea level,
/// whatever the ground: a pixel on the northern cap converts to where the
/// sea-level sphere meets its line of sight, and back, though the ground
/// around stands 1000 m high and the center with it.
#[test]
#[ignore = "the globe's ground_under doesn't meet the polar caps where the ground's heights leave out sea level"]
fn a_pixel_on_a_polar_cap_converts_on_sea_level() {
    let run_loop = Queue::<()>::new();
    let globe_style = r#"{"version": 8, "center": [10, 80], "zoom": 2, "sources": {}, "layers": [],
        "projection": {"type": "globe"}}"#;
    let camera = CameraOptions { center: Some(LatLng::new(80.0, 10.0).unwrap()), zoom: Some(2.0), ..pitched(0.0) };
    let mut map = map_over(globe_style, None, false, camera, &run_loop);
    map.jump_to(&CameraOptions { center_altitude: Some(1000.0), ..Default::default() });
    let view = map.snapshot().view;
    assert!(map.snapshot().globe_rendering);
    let flat_globe = MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap();
    let globe =
        MapProjection::from_view(&view, &WorldMapping::Mercator).unwrap().with_ground(Some(flat(Some(1000.0), 1000.0)));

    let mut on_the_cap = 0;
    for pixel in (0..16).flat_map(|row| {
        (0..16).map(move |column| ScreenCoordinate::new(16.0 + 32.0 * f64::from(column), 16.0 + 32.0 * f64::from(row)))
    }) {
        let Ok(sea_level) = flat_globe.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped) else { continue };
        if sea_level.latitude() < mln_style::constants::LATITUDE_MAX + 0.5
            || flat_globe.is_location_occluded(&sea_level)
        {
            continue;
        }
        on_the_cap += 1;
        let location = globe.lat_lng_for_pixel(&pixel, WrapMode::Unwrapped).unwrap();
        assert_same_place(sea_level, location, 1e-9, &format!("{pixel:?}"));
        assert_same_pixel(pixel, globe.pixel_for_lat_lng(&location), 0.01, "back on the cap");
    }
    assert!(on_the_cap > 4, "{on_the_cap} pixels on the cap");
}

/// Bounds 0.1° on a side north east of the center.
fn bounds() -> LatLngBounds {
    LatLngBounds::hull(LatLng::new(20.02, 10.03).unwrap(), LatLng::new(20.12, 10.13).unwrap())
}

/// Over terrain, a fit doesn't depend on the altitude the camera stands at:
/// from a camera on ground 2000 m high, it is the fit from one at sea level,
/// at any bearing and pitch. Once a camera that looks straight down moves
/// there and stands on that ground, the bounds' corners, as high as the
/// center, touch the padding and stay inside it; and the bounds the camera
/// shows fit back to it.
#[test]
fn a_fit_over_terrain_shows_the_locations_as_high_as_the_center_inside_the_padding() {
    let run_loop = Queue::<()>::new();
    let padding = EdgeInsets::new(40.0, 30.0, 20.0, 10.0);
    let high = flat(Some(2000.0), 2000.0);
    for (from, bearing, pitch) in
        [(0.0, None, None), (40.0, Some(30.0), Some(0.0)), (40.0, None, None), (0.0, Some(-20.0), Some(50.0))]
    {
        let what = format!("from pitch {from}, bearing {bearing:?}, pitch {pitch:?}");
        let mut map = map_over(STYLE, Some(&high), true, pitched(from), &run_loop);
        assert_eq!(2000.0, map.transform_state().view().center_altitude());
        let at_sea_level = map_over(STYLE, Some(&flat(Some(0.0), 0.0)), true, pitched(from), &run_loop);
        let fit = map.camera_for_lat_lng_bounds(&bounds(), &padding, bearing, pitch).unwrap();
        let expected = at_sea_level.camera_for_lat_lng_bounds(&bounds(), &padding, bearing, pitch).unwrap();
        assert_same_place(expected.center.unwrap(), fit.center.unwrap(), 1e-9, &what);
        assert_near(expected.zoom.unwrap(), fit.zoom.unwrap(), 1e-9, &what);

        map.jump_to(&fit);
        assert_eq!(2000.0, map.transform_state().view().center_altitude(), "{what}");
        // A pitched view's fit is the flat map's estimate, as in GL JS.
        if map.transform_state().view().pitch() != 0.0 {
            continue;
        }
        let corners = [bounds().northwest(), bounds().northeast(), bounds().southeast(), bounds().southwest()];
        let pixels = corners.map(|corner| map.pixel_for_lat_lng(&corner));
        let (left, top) = (padding.left(), padding.top());
        let (right, bottom) = (512.0 - padding.right(), 512.0 - padding.bottom());
        for pixel in pixels {
            let inside =
                (left - 1e-6..=right + 1e-6).contains(&pixel.x) && (top - 1e-6..=bottom + 1e-6).contains(&pixel.y);
            assert!(inside, "{what}: {pixel:?} outside the padding");
        }
        let touches = |value: f64, edge: f64| (value - edge).abs() < 1e-6;
        let touching = pixels
            .iter()
            .filter(|pixel| {
                touches(pixel.x, left) || touches(pixel.x, right) || touches(pixel.y, top) || touches(pixel.y, bottom)
            })
            .count();
        assert!(touching > 0, "{what}: no corner touches the padding: {pixels:?}");
    }

    let map = map_over(STYLE, Some(&high), true, CameraOptions::default(), &run_loop);
    let camera = map.transform_state().camera_options(None);
    let shown = map.lat_lng_bounds_for_camera(&camera).unwrap();
    let fit = map.camera_for_lat_lng_bounds(&shown, &EdgeInsets::default(), None, None).unwrap();
    assert_same_place(camera.center.unwrap(), fit.center.unwrap(), 1e-9, "the shown bounds' fit");
    assert_near(camera.zoom.unwrap(), fit.zoom.unwrap(), 1e-9, "the shown bounds' fit");
}
