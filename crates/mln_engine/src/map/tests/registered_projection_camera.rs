//! A map in a registered projection, as its views, conversions and camera
//! helpers see it.

use mln_style::error::ErrorKind;
use mln_style::geo::{LatLng, LatLngBounds, WrapMode};
use mln_style::size::Size;

use crate::camera::{BoundOptions, CameraOptions, ConstrainMode, MapProjection};
use crate::exec::queue::Queue;
use crate::geo::projection::Projection;
use crate::geo::world::WorldMapping;
use crate::geo::{EdgeInsets, ScreenCoordinate};
use crate::map::{Map, MapOptions};
use crate::storage::stub_file_source::{ResponseType, StubFileSource};
use crate::tests::{crs, registry_builder};

/// A map of `size` pixels square whose registry has the built-in projections
/// and `extra`.
fn map(size: u32, extra: Vec<mln_style::crs::Crs>, run_loop: &Queue<()>) -> Map {
    let mut builder = registry_builder();
    for crs in extra {
        builder.add_projection(crs).unwrap();
    }
    Map::with_file_source(
        StubFileSource::on_queue(run_loop, ResponseType::Asynchronous),
        MapOptions { size: Size::new(size, size), ..Default::default() },
        builder.build(),
        &crate::map::tests::pool(),
    )
    .expect("map")
}

/// A style without sources in the projection `projection`, centered on
/// `[lng, lat]` at `zoom`.
fn style(projection: &str, [lng, lat]: [f64; 2], zoom: f64) -> String {
    format!(
        r#"{{"version":8,"projection":{{"type":"{projection}"}},"center":[{lng},{lat}],"zoom":{zoom},"sources":{{}},"layers":[]}}"#
    )
}

fn ll(lat: f64, lng: f64) -> LatLng {
    LatLng::new(lat, lng).unwrap()
}

#[track_caller]
fn assert_near(expected: f64, actual: f64, tolerance: f64) {
    assert!((expected - actual).abs() <= tolerance, "{actual} is not within {tolerance} of {expected}");
}

#[test]
fn a_projection_from_a_view_converts_in_the_maps_registered_projection() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::north_polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [45.0, 70.0], 3.0));
    map.jump_to(&CameraOptions { bearing: Some(20.0), pitch: Some(30.0), ..CameraOptions::default() });

    let view = map.snapshot().view;
    let world = map.transform_state().view().world().clone();
    let projection = MapProjection::from_view(&view, &world).unwrap();
    let from_map = MapProjection::new(&map);
    for lat_lng in [ll(70.0, 45.0), ll(75.0, 10.0), ll(66.0, 80.0)] {
        let expected = from_map.pixel_for_lat_lng(&lat_lng);
        let actual = projection.pixel_for_lat_lng(&lat_lng);
        assert!((actual.x - expected.x).abs() < 1e-6 && (actual.y - expected.y).abs() < 1e-6, "{lat_lng:?}");
        let back = projection.lat_lng_for_pixel(&actual, WrapMode::Unwrapped).unwrap();
        assert_near(lat_lng.latitude(), back.latitude(), 1e-9);
        assert_near(lat_lng.longitude(), back.longitude(), 1e-9);
    }
    assert_eq!(map.snapshot().view, view.to_view(&world).unwrap().map_view(view.extent.scale_factor));
}

#[test]
fn a_view_of_the_identity_places_its_center_in_the_identity_square() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, Vec::new(), &run_loop);
    map.load_style_json(&style("identity", [45.0, 45.0], 1.0));

    // Tile 1/1/0 of the identity spans lng 0..90 and lat 0..90, and fills
    // the view; Mercator would put 45° N well above the view's middle.
    let projection = MapProjection::from_view(&map.snapshot().view, map.transform_state().view().world()).unwrap();
    let middle = projection.pixel_for_lat_lng(&ll(45.0, 45.0));
    assert_near(256.0, middle.x, 1e-9);
    assert_near(256.0, middle.y, 1e-9);
    let top_left = projection.lat_lng_for_pixel(&ScreenCoordinate::new(0.0, 0.0), WrapMode::Unwrapped).unwrap();
    assert_near(90.0, top_left.latitude(), 1e-6);
    assert_near(0.0, top_left.longitude(), 1e-6);
}

#[test]
fn a_view_whose_center_the_projection_has_no_place_for_is_refused() {
    let northern = crs::crs(
        "test:northern",
        |[lng, lat]| if lat < 0.0 { [f64::NAN; 2] } else { [lng, lat] },
        |position| position,
        [-90.0, 90.0],
        180.0,
    );
    let run_loop = Queue::<()>::new();
    let map = map(256, Vec::new(), &run_loop);
    let mut view = map.snapshot().view;
    view.camera.center = ll(-10.0, 0.0);

    let error = MapProjection::from_view(&view, &crs::mapping(northern)).err().map(|error| error.kind);
    assert_eq!(Some(ErrorKind::InvalidArgument), error);
}

/// A feature of tile `z/x/y` of type `type_`, whose rings are `rings` in
/// tile coordinates.
struct TileFeature {
    type_: mln_style::feature::FeatureType,
    rings: mln_style::feature::GeometryCollection,
}

impl mln_style::feature::GeometryTileFeature for TileFeature {
    fn get_type(&self) -> mln_style::feature::FeatureType {
        self.type_
    }

    fn get_value(&self, _key: &str) -> Option<mln_style::feature::Value> {
        None
    }

    fn get_geometries(
        &self,
    ) -> Result<std::borrow::Cow<'_, mln_style::feature::GeometryCollection>, mln_style::error::Error> {
        Ok(std::borrow::Cow::Borrowed(&self.rings))
    }
}

fn tile_feature(type_: mln_style::feature::FeatureType, rings: &[&[(i16, i16)]]) -> TileFeature {
    let rings = rings.iter().map(|ring| ring.iter().map(|&(x, y)| geometry::Point::new(x, y)).collect::<Vec<_>>());
    TileFeature { type_, rings: mln_style::feature::GeometryCollection::from(rings.collect::<Vec<_>>()) }
}

/// GL JS's "query returns geometry through the transform projection"
/// (`src/data/feature_index.test.ts`): the top left of tile 2/1/2 is a
/// quarter of the way across the identity's square and halfway down, at
/// 45° W on the equator; Web Mercator puts it at 90° W.
#[test]
fn a_queried_feature_has_the_location_the_identity_gives_its_tile_coordinates() {
    use mln_style::feature::{FeatureType, GeometryTileFeature};

    let tile = mln_style::tile_id::CanonicalTileID::new(2, 1, 2);
    let point = tile_feature(FeatureType::Point, &[&[(0, 0)]]);
    let converted =
        crate::tile::convert_feature(&point, &point.get_geometries().unwrap(), &tile, &crs::identity()).unwrap();
    assert_eq!(Some(geo_types::Geometry::Point(geo_types::Point::new(-45.0, 0.0))), converted.geometry);
    let mercator =
        crate::tile::convert_feature(&point, &point.get_geometries().unwrap(), &tile, &WorldMapping::Mercator).unwrap();
    let Some(geo_types::Geometry::Point(mercator)) = mercator.geometry else { panic!("a point: {mercator:?}") };
    assert_eq!(-90.0, mercator.x());
    assert_near(0.0, mercator.y(), 1e-12);

    // A square with a square hole, its rings in order, each corner where the
    // identity puts it: a tile's 8192 units are 45 degrees at zoom 2.
    let square = &[(0, 0), (8192, 0), (8192, 8192), (0, 8192), (0, 0)][..];
    let hole = &[(2048, 2048), (2048, 6144), (6144, 6144), (6144, 2048), (2048, 2048)][..];
    let polygon = tile_feature(FeatureType::Polygon, &[square, hole]);
    let converted =
        crate::tile::convert_feature(&polygon, &polygon.get_geometries().unwrap(), &tile, &crs::identity()).unwrap();
    let Some(geo_types::Geometry::Polygon(converted)) = converted.geometry else {
        panic!("a polygon: {converted:?}");
    };
    let lng_lats =
        |ring: &geo_types::LineString<f64>| ring.0.iter().map(|coord| (coord.x, coord.y)).collect::<Vec<_>>();
    assert_eq!(
        vec![(-45.0, 0.0), (0.0, 0.0), (0.0, -45.0), (-45.0, -45.0), (-45.0, 0.0)],
        lng_lats(converted.exterior())
    );
    assert_eq!(
        vec![(-33.75, -11.25), (-33.75, -33.75), (-11.25, -33.75), (-11.25, -11.25), (-33.75, -11.25)],
        lng_lats(&converted.interiors()[0])
    );
}

/// GL JS's "fits the cap north of 60N with the pole at the center"
/// (`src/ui/map_tests/map_projection.test.ts`): the cap's corners are the
/// pole and two points on its edge, but its world box is the square around
/// the pole.
#[test]
fn the_camera_for_a_polar_cap_centers_on_the_pole() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [0.0, 70.0], 2.0));

    let cap = LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0));
    let camera = map.camera_for_lat_lng_bounds(&cap, &EdgeInsets::default(), None, None).unwrap();
    assert_near(90.0, camera.center.unwrap().latitude(), 1e-6);

    // The fitted camera shows the whole cap: its edge at 60° N, down the
    // view along the projection's central meridian, is just inside the
    // view's bottom edge, as far inside as the box grew by how far its
    // edges curve between samples.
    map.jump_to(&camera);
    let edge = map.pixel_for_lat_lng(&ll(60.0, -45.0));
    assert!(500.0 < edge.y && edge.y <= 512.0, "{edge:?}");
}

/// A converter that records how many positions each call of its `forward`
/// takes.
struct CountedForward {
    converter: Box<dyn mln_style::crs::CrsConverter>,
    calls: std::sync::Arc<parking_lot::Mutex<Vec<usize>>>,
}

impl mln_style::crs::CrsConverter for CountedForward {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        self.calls.lock().push(lng_lats.len());
        self.converter.forward(lng_lats, positions);
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        self.converter.inverse(positions, lng_lats);
    }
}

// gtest: Map.LatLngBoundsToCameraInAPolarProjection
// The box is fitted by its samples, the 17 by 17 grid and the halfway
// points of its edges, which go through the converter in one call.
#[test]
fn a_box_is_fitted_by_its_samples_in_one_batch() {
    let run_loop = Queue::<()>::new();
    let calls = std::sync::Arc::default();
    let polar = crs::north_polar_stereographic("test:polar-fit");
    let converter = Box::new(CountedForward { converter: polar.converter, calls: std::sync::Arc::clone(&calls) });
    let mut map = map(512, vec![mln_style::crs::Crs { converter, ..polar }], &run_loop);
    map.load_style_json(&style("test:polar-fit", [0.0, 70.0], 2.0));
    calls.lock().clear();

    // Its corners are the pole and one point on its edge, but the cap north
    // of 60 N is a disc around the pole.
    let cap = LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0));
    let camera = map.camera_for_lat_lng_bounds(&cap, &EdgeInsets::default(), None, None).unwrap();
    assert_near(90.0, camera.center.unwrap().latitude(), 1e-6);
    let calls = calls.lock();
    assert!(calls.contains(&(17 * 17 + 4 * 16)), "{calls:?}");
    assert!(calls.len() < 17 * 17, "{calls:?}");
}

/// GL JS fits a box at a bearing by its samples turned with the view: a
/// polar cap, round about its pole, fits at the same zoom at any bearing,
/// where the corners of its world box would stick out at 45° and fit it
/// half a zoom level out.
#[test]
fn the_camera_for_a_polar_cap_fits_it_at_any_bearing() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [0.0, 70.0], 2.0));

    let cap = LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0));
    let at = |bearing: f64| map.camera_for_lat_lng_bounds(&cap, &EdgeInsets::default(), Some(bearing), None).unwrap();
    let (north_up, turned) = (at(0.0), at(45.0));
    assert_near(north_up.zoom.unwrap(), turned.zoom.unwrap(), 1e-9);
    assert_near(45.0, turned.bearing.unwrap(), 1e-9);
    assert_near(90.0, turned.center.unwrap().latitude(), 1e-6);
}

/// A box of the identity is the rectangle of its corners: lng 0..90 and lat
/// 0..90, half the square across, fill a 512-pixel view at zoom 1.
#[test]
fn the_camera_for_a_box_of_the_identity_fills_the_view_with_it() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, Vec::new(), &run_loop);
    map.load_style_json(&style("identity", [0.0, 0.0], 0.0));

    let quarter = LatLngBounds::hull(ll(0.0, 0.0), ll(90.0, 90.0));
    let camera = map.camera_for_lat_lng_bounds(&quarter, &EdgeInsets::default(), None, None).unwrap();
    assert_near(45.0, camera.center.unwrap().latitude(), 1e-9);
    assert_near(45.0, camera.center.unwrap().longitude(), 1e-9);
    assert_near(1.0, camera.zoom.unwrap(), 1e-6);
}

/// The view's corners in a registered projection: north is up at the
/// center's meridian only, so the area's eastmost location is at the view's
/// top-right corner, not at its bottom-right one.
#[test]
fn the_bounds_a_camera_shows_hold_every_corner_of_the_view() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::north_polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [0.0, 70.0], 3.0));

    let bounds = map.lat_lng_bounds_for_camera(&CameraOptions::default()).unwrap();
    for (x, y) in [(0.0, 0.0), (512.0, 0.0), (512.0, 512.0), (0.0, 512.0)] {
        let corner = map.lat_lng_for_pixel(&ScreenCoordinate::new(x, y), WrapMode::Unwrapped).unwrap();
        assert!(bounds.contains(&corner), "{corner:?} isn't in {bounds:?}");
    }
    let top_right = map.lat_lng_for_pixel(&ScreenCoordinate::new(512.0, 0.0), WrapMode::Unwrapped).unwrap();
    let bottom_right = map.lat_lng_for_pixel(&ScreenCoordinate::new(512.0, 512.0), WrapMode::Unwrapped).unwrap();
    assert!(top_right.longitude() > bottom_right.longitude(), "{top_right:?} {bottom_right:?}");
    assert_eq!(bounds, map.lat_lng_bounds_for_camera_unwrapped(&CameraOptions::default()).unwrap());
}

/// Max bounds keep the center inside them, as Mercator's do: at zoom 4 the
/// identity's 180 degrees are 8192 pixels, and a 200-pixel view centered on
/// the bounds' corner reaches 100 pixels past it. GL JS keeps the whole view
/// inside them, which `ConstrainMode::Screen` does
/// (`src/camera/mercator_camera_math.test.ts`, "keeps the viewport inside
/// explicit max bounds set inside the square").
#[test]
fn max_bounds_inside_the_identity_keep_the_center_or_with_screen_the_view() {
    for (mode, corner) in [(ConstrainMode::HeightOnly, 10.0), (ConstrainMode::Screen, 10.0 - 100.0 * 180.0 / 8192.0)] {
        let run_loop = Queue::<()>::new();
        let options = MapOptions { size: Size::new(200, 200), constrain_mode: mode, ..Default::default() };
        let mut map = Map::with_file_source(
            StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous),
            options,
            crate::tests::registry(),
            &crate::map::tests::pool(),
        )
        .unwrap();
        map.load_style_json(&style("identity", [0.0, 0.0], 4.0));
        let bounds = LatLngBounds::hull(ll(-10.0, -10.0), ll(10.0, 10.0));
        map.set_bounds(&BoundOptions { bounds: Some(bounds), ..BoundOptions::default() }).unwrap();

        map.jump_to(&CameraOptions { center: Some(ll(80.0, 80.0)), ..CameraOptions::default() });
        let center = map.transform_state().view().center(WrapMode::Unwrapped);
        assert_near(corner, center.latitude(), 1e-6);
        assert_near(corner, center.longitude(), 1e-6);
    }
}

/// GL JS's "keeps a center on the far side of the pole inside maxBounds of
/// the cap north of 60N" (`src/ui/map_tests/map_projection.test.ts`).
#[test]
fn a_center_beyond_the_pole_stays_inside_the_max_bounds_of_a_polar_cap() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [0.0, 70.0], 3.0));
    let cap = LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0));
    map.set_bounds(&BoundOptions { bounds: Some(cap), ..BoundOptions::default() }).unwrap();

    map.jump_to(&CameraOptions { center: Some(ll(70.0, 45.0)), ..CameraOptions::default() });
    let center = map.transform_state().view().center(WrapMode::Unwrapped);
    assert_near(70.0, center.latitude(), 1e-6);
    assert_near(45.0, center.longitude(), 1e-6);
}

/// Meters per pixel as GL JS's geolocate control measures them in a
/// registered projection: the ground distance from a location to the place
/// 100 pixels from it along the projection's x axis, by GL JS's
/// `LngLat.distanceTo`, over 100, whichever way the camera looks. The numbers
/// are GL JS's for its test polar stereographic projection (MapLibre
/// Native's `Map.MetersPerPixelInARegisteredProjection`). Mercator keeps its
/// scale at the latitude.
#[test]
fn meters_per_pixel_are_the_projections() {
    let run_loop = Queue::<()>::new();
    let mut mercator = map(512, Vec::new(), &run_loop);
    mercator.load_style_json(&style("mercator", [10.0, 40.0], 3.5));
    let mercator = MapProjection::new(&mercator);
    for latitude in [0.0, 40.0, -70.0] {
        let scale = Projection::meters_per_pixel_at_latitude(latitude, 3.5);
        assert_eq!(scale, mercator.meters_per_pixel_at_latitude(latitude));
        assert_eq!(scale, mercator.meters_per_pixel_at(&ll(latitude, 20.0)));
    }

    let gl_js = |expected: f64, actual: f64| {
        assert!(((actual - expected) / expected).abs() < 1e-9, "{actual} is not GL JS's {expected}");
    };
    let mut map = map(512, vec![crs::polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("test:polar", [-45.0, 70.0], 4.0));
    let camera = |center, zoom, bearing, pitch| CameraOptions {
        center: Some(center),
        zoom: Some(zoom),
        bearing: Some(bearing),
        pitch: Some(pitch),
        ..CameraOptions::default()
    };
    let cameras = [
        (ll(70.0, -45.0), 4.0, ll(70.0, -45.0), 1_022.832_584_048_677_6),
        (ll(80.0, 30.0), 6.0, ll(81.0, 40.0), 261.998_661_983_620_9),
        (ll(60.0, -100.0), 8.0, ll(60.1, -100.2), 61.535_774_394_909_105),
    ];
    for (center, zoom, location, meters) in cameras {
        map.jump_to(&camera(center, zoom, 0.0, 0.0));
        gl_js(meters, MapProjection::new(&map).meters_per_pixel_at(&location));
    }
    let (center, zoom, location, meters) = cameras[0];
    map.jump_to(&camera(center, zoom, 30.0, 45.0));
    gl_js(meters, MapProjection::new(&map).meters_per_pixel_at(&location));
    // At the latitude, on the center's meridian.
    gl_js(meters, MapProjection::new(&map).meters_per_pixel_at_latitude(location.latitude()));
}

/// GL JS's "setProjection accepts the registered name"
/// (`src/ui/map_tests/map_projection.test.ts`), from Rust: a style edit
/// sets the registered projection by its name, as it sets a built-in one,
/// and the map draws in it, its center where it was.
#[test]
fn a_registered_projection_set_from_rust_draws_the_map_in_it() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, vec![crs::north_polar_stereographic("test:polar")], &run_loop);
    map.load_style_json(&style("mercator", [45.0, 70.0], 3.0));
    assert!(map.transform_state().view().world().wraps());

    let mut projection = mln_style::projection::Projection::new();
    projection.set_registered_name("test:polar").unwrap();
    map.edit_style(|style| style.apply(mln_style::Edit::SetProjection(projection))).unwrap();

    let view = map.transform_state().view();
    assert_eq!("test:polar", view.world().name());
    assert_eq!(Some("test:polar"), map.style().get_projection().registered_name());
    assert_near(70.0, view.center(WrapMode::Unwrapped).latitude(), 1e-9);
    assert_near(45.0, view.center(WrapMode::Unwrapped).longitude(), 1e-9);
}

/// Over terrain in a registered projection, the ground a screen point shows
/// is located through the projection, and a location stands on the ground
/// drawn at its place in the projection's world square: here ground that
/// rises eastward across the identity's square.
#[test]
fn ground_hits_are_located_through_the_registered_projection() {
    let run_loop = Queue::<()>::new();
    let mut map = map(512, Vec::new(), &run_loop);
    map.load_style_json(&style("identity", [45.0, 45.0], 1.0));
    map.jump_to(&CameraOptions { pitch: Some(50.0), ..CameraOptions::default() });
    // The identity's world is 180 m wide: the eye is about 90 m up.
    let ground = super::ground::Slope { x: 0.5, height: 10.0, rise: 20.0 };
    let view = map.transform_state().view();

    for (x, y) in [(256.0, 256.0), (100.0, 150.0), (400.0, 60.0)] {
        // Measured from the bottom edge.
        let point = ScreenCoordinate::new(x, y);
        let hit = view.ground_under(&point, &ground).expect("the ground");
        let location = view.screen_coordinate_to_lat_lng_on(&point, &ground, WrapMode::Unwrapped).unwrap();
        let expected = view.world().lat_lng_from_world(&geometry::Point::new(hit.world.x, hit.world.y)).unwrap();
        assert_eq!(expected, location);
        assert_eq!(expected, hit.location(view.world(), WrapMode::Unwrapped));

        let drawn = view.drawn_height_at(&location, &ground).unwrap();
        assert_near(hit.height, drawn, 1e-3);
        let back = view.lat_lng_to_screen_coordinate_on(&location, &ground);
        assert_near(x, back.x, 1e-4);
        assert_near(y, back.y, 1e-4);
    }
}
