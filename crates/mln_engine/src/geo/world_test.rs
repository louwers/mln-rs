//! The world square's mappings: MapLibre Native's
//! `test/map/world_coordinate_helper.test.cpp` and GL JS's
//! `src/geo/projection/crs.test.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use geometry::Point;
use mln_style::constants::LATITUDE_MAX;
use mln_style::crs::{Crs, CrsConverter};
use mln_style::geo::{LatLng, LatLngBounds};

use super::{CrsMapping, WorldMapping};
use crate::geo::constants::EARTH_RADIUS_M;
use crate::tests::crs::{self, crs, identity, mapping};

fn ll(lat: f64, lng: f64) -> LatLng {
    LatLng::new(lat, lng).unwrap()
}

fn lat_lng(world: &WorldMapping, x: f64, y: f64) -> LatLng {
    world.lat_lng_from_world(&Point::new(x, y)).unwrap()
}

#[track_caller]
fn assert_near(expected: f64, actual: f64, tolerance: f64) {
    assert!((expected - actual).abs() <= tolerance, "{actual} is not within {tolerance} of {expected}");
}

fn crs_mapping(world: &WorldMapping) -> &CrsMapping {
    match world {
        WorldMapping::Crs(crs) => crs,
        WorldMapping::Mercator => panic!("Mercator has no CRS"),
    }
}

/// GL JS's sample locations, `[lng, lat]`.
const SAMPLES: [[f64; 2]; 6] =
    [[0.0, 0.0], [-89.0, -89.0], [89.0, 89.0], [12.5, 41.9], [-73.98, 40.75], [174.77, -41.29]];

/// The identity, with an inverse that answers only inside its tile matrix
/// set, -90 to 90 on both axes.
fn identity_answering_inside() -> WorldMapping {
    mapping(crs(
        "test:identity-answering-inside",
        |position| position,
        |[x, y]| if x.abs() <= 90.0 && y.abs() <= 90.0 { [x, y] } else { [f64::NAN; 2] },
        [-90.0, 90.0],
        180.0,
    ))
}

/// GL JS's CRS rotated by 30 degrees.
fn rotated() -> WorldMapping {
    let (sin, cos) = (std::f64::consts::PI / 6.0).sin_cos();
    mapping(crs(
        "test:rotated",
        move |[lng, lat]| [lng * cos - lat * sin, lng * sin + lat * cos],
        move |[x, y]| [x * cos + y * sin, -x * sin + y * cos],
        [-150.0, 150.0],
        300.0,
    ))
}

/// Spherical Mercator written as a CRS, so its world coordinates can be
/// checked against Mercator's own.
fn mercator_as_crs() -> WorldMapping {
    let half_circumference = std::f64::consts::PI * EARTH_RADIUS_M;
    mapping(crs(
        "test:mercator-as-crs",
        |[lng, lat]| {
            let phi = lat.to_radians();
            [lng.to_radians() * EARTH_RADIUS_M, EARTH_RADIUS_M * (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln()]
        },
        |[x, y]| {
            [
                (x / EARTH_RADIUS_M).to_degrees(),
                (2.0 * (y / EARTH_RADIUS_M).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees(),
            ]
        },
        [-half_circumference, half_circumference],
        2.0 * half_circumference,
    ))
}

// gtest: CRSWorldCoordinateHelper.Identity
#[test]
fn the_identity_takes_lng_lat_as_its_coordinates() {
    let world = identity();
    assert_eq!("identity", world.name());
    assert!(!world.wraps());
    assert_eq!(Point::new(0.75, 0.25), world.world_from_lat_lng(&ll(45.0, 45.0)));
    assert_eq!(ll(45.0, 45.0), lat_lng(&world, 0.75, 0.25));
    assert_eq!(180.0, world.meters_per_world_unit(&Point::new(0.5, 0.5)));
}

// GL JS: identity maps the tile 0 square to lng/lat -90..90.
#[test]
fn the_identity_maps_the_tile_0_square_to_90_degrees_each_way() {
    let world = identity();
    assert_eq!(Point::new(0.0, 0.0), world.world_from_lat_lng(&ll(90.0, -90.0)));
    assert_eq!(Point::new(1.0, 1.0), world.world_from_lat_lng(&ll(-90.0, 90.0)));
    assert_eq!(Point::new(0.5, 0.5), world.world_from_lat_lng(&ll(0.0, 0.0)));
}

// GL JS: identity round trips lng/lat through world coordinates.
#[test]
fn the_identity_round_trips_lng_lat() {
    let world = identity();
    for [lng, lat] in SAMPLES {
        let back = world.lat_lng_from_world(&world.world_from_lat_lng(&ll(lat, lng))).unwrap();
        assert_near(lng, back.longitude(), 5e-13);
        assert_near(lat, back.latitude(), 5e-13);
    }
}

// GL JS: identity uses a constant meters per world unit equal to the zoom 0
// extent wherever the location is.
#[test]
fn the_identity_takes_its_extent_as_meters_per_world_unit_everywhere() {
    let world = identity();
    assert_eq!(180.0, world.meters_per_world_unit(&Point::new(0.5, 0.5)));
    assert_eq!(180.0, world.meters_per_world_unit(&Point::new(0.95, 0.05)));
}

// gtest: CRSWorldCoordinateHelper.LatLngPastTheEdgeOfTheSquareIsUnclamped
#[test]
fn a_lat_lng_past_the_edge_of_the_square_is_unclamped() {
    let world = identity();
    // The identity has an answer everywhere: a position east of the square
    // keeps its own longitude.
    assert_eq!(ll(0.0, 135.0), lat_lng(&world, 1.25, 0.5));
    assert!(world.has_lat_lng(&Point::new(1.25, 0.5)));
    assert_eq!(ll(90.0, 0.0), lat_lng(&world, 0.5, -0.5));
}

// GL JS: identity clamps a latitude past the poles to the range LngLat
// accepts, for a position outside the world square.
#[test]
fn a_latitude_past_the_poles_is_clamped() {
    let world = identity();
    assert_eq!(90.0, lat_lng(&world, 0.5, -0.1).latitude());
    assert_eq!(-90.0, lat_lng(&world, 0.5, 1.1).latitude());
    assert!(!world.has_lat_lng(&Point::new(0.5, -0.1)));
}

// gtest: CRSWorldCoordinateHelper.PositionWithoutAnAnswerUsesTheEdgeTowardTheCenter
#[test]
fn a_position_without_an_answer_uses_the_edge_toward_the_center() {
    let world = mapping(crs::disc("test:disc"));
    assert!(!world.has_lat_lng(&Point::new(1.0, 1.0)));
    assert!(world.has_lat_lng(&Point::new(0.5, 0.5)));
    // The corner of the square has no answer, nor any position past it; the
    // last one with an answer on the line to the center is on the disc.
    let edge = lat_lng(&world, 2.0, 2.0);
    assert_near(90.0, edge.longitude().hypot(edge.latitude()), 1e-6);
    assert_near(-edge.latitude(), edge.longitude(), 1e-9);
    // A position past the square whose clamped one has an answer uses that
    // one.
    assert_eq!(ll(0.0, 90.0), lat_lng(&world, 1.5, 0.5));
}

/// A converter that counts its calls of `forward`.
struct CountingForward {
    converter: Box<dyn CrsConverter>,
    forward_calls: Arc<AtomicUsize>,
}

impl CrsConverter for CountingForward {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        self.forward_calls.fetch_add(1, Ordering::SeqCst);
        self.converter.forward(lng_lats, positions);
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        self.converter.inverse(positions, lng_lats);
    }
}

// gtest: CRSWorldCoordinateHelper.KeepsAHoleInABatchWhileItsNeighborsConvert
#[test]
fn a_hole_in_a_batch_stays_while_its_neighbors_convert() {
    let Crs { converter, tile_matrix_set, .. } = crs::disc("test:batch");
    let forward_calls = Arc::new(AtomicUsize::new(0));
    let converter = Box::new(CountingForward { converter, forward_calls: Arc::clone(&forward_calls) });
    let world = mapping(Crs { name: "test:batch".to_owned(), converter, tile_matrix_set });

    let mut places = [[0.0; 2]; 3];
    world.world_from_lng_lats(&[[-45.0, 45.0], [f64::NAN, f64::NAN], [45.0, -45.0]], &mut places);
    assert_eq!(1, forward_calls.load(Ordering::SeqCst));
    assert_eq!([0.25, 0.25], places[0]);
    assert!(places[1][0].is_nan() && places[1][1].is_nan());
    assert_eq!([0.75, 0.75], places[2]);

    // The corner between them has no answer of its own and falls back to the
    // edge of the disc, while they keep theirs.
    let mut answers = [[0.0; 2]; 3];
    world.lng_lats_from_world(&[[0.25, 0.25], [1.0, 1.0], [0.75, 0.75]], &mut answers);
    assert_eq!([-45.0, 45.0], answers[0]);
    assert_near(90.0, answers[1][0].hypot(answers[1][1]), 1e-6);
    assert_near(-answers[1][1], answers[1][0], 1e-9);
    assert_eq!([45.0, -45.0], answers[2]);
}

// GL JS: maps a position past the east edge to the nearest position inside
// where inverse returns NaN.
#[test]
fn a_position_past_the_east_edge_takes_the_nearest_answer_inside() {
    assert_eq!(ll(45.0, 90.0), lat_lng(&identity_answering_inside(), 1.5, 0.25));
}

// GL JS: maps a position past the edge of the world to the edge where
// inverse wraps it to the other side.
#[test]
fn a_position_past_the_edge_of_the_world_takes_the_edge_where_the_inverse_wraps() {
    let world = mapping(crs(
        "test:wrapping-world",
        |position| position,
        |[x, y]| [(x + 540.0) % 360.0 - 180.0, y],
        [-360.0, 360.0],
        720.0,
    ));
    let past_the_east_edge = lat_lng(&world, 0.875, 0.5);
    assert_near(180.0, past_the_east_edge.longitude(), 5e-6);
    assert_near(0.0, past_the_east_edge.latitude(), 5e-6);
}

// GL JS: keeps the inverse of a converter that is only approximately
// invertible.
#[test]
fn a_nearly_invertible_converter_keeps_its_inverse() {
    let world = mapping(crs("test:approximate", |position| position, |[x, y]| [x * 1.0001, y], [-90.0, 90.0], 180.0));
    assert_near(45.0045, lat_lng(&world, 0.75, 0.5).longitude(), 5e-7);
}

// GL JS: keeps the inverse of a converter that never leads back to the
// position.
#[test]
fn a_converter_that_never_leads_back_keeps_its_own_answer() {
    let world = mapping(crs("test:imprecise", |position| position, |[x, y]| [x + 1.0, y], [-90.0, 90.0], 180.0));
    assert_eq!(ll(0.0, 1.0), lat_lng(&world, 0.5, 0.5));
}

// gtest: CRSWorldCoordinateHelper.ThrowsWhereTheInverseNeverAnswers
#[test]
fn a_position_has_no_location_where_the_inverse_never_answers() {
    let world = mapping(crs("test:never", |position| position, |_| [f64::NAN; 2], [-90.0, 90.0], 180.0));
    assert!(world.lat_lng_from_world(&Point::new(0.5, 0.5)).is_err());
    let mut lng_lats = [[0.0; 2]];
    world.lng_lats_from_world(&[[0.5, 0.5]], &mut lng_lats);
    assert!(lng_lats[0][0].is_nan() && lng_lats[0][1].is_nan());
}

// GL JS: rotated CRS round trips lng/lat, measures world coordinates from the
// tile matrix set's origin with y growing down, and takes its zoom 0 extent
// as meters per world unit.
#[test]
fn a_rotated_crs_measures_from_its_origin_with_y_down() {
    let world = rotated();
    for [lng, lat] in SAMPLES {
        let back = world.lat_lng_from_world(&world.world_from_lat_lng(&ll(lat, lng))).unwrap();
        assert_near(lng, back.longitude(), 5e-11);
        assert_near(lat, back.latitude(), 5e-11);
    }
    let location = lat_lng(&world, 0.25, 0.75);
    let mut position = [[0.0; 2]];
    crs_mapping(&world).crs().converter.forward(&[[location.longitude(), location.latitude()]], &mut position);
    assert_near(-150.0 + 0.25 * 300.0, position[0][0], 5e-11);
    assert_near(150.0 - 0.75 * 300.0, position[0][1], 5e-11);
    assert_eq!(300.0, world.meters_per_world_unit(&Point::new(0.5, 0.5)));
}

// GL JS: Mercator written as a CRS projects and unprojects as Mercator does,
// and has its meters per world unit at the equator.
#[test]
fn mercator_written_as_a_crs_maps_as_mercator() {
    let world = mercator_as_crs();
    let mercator = WorldMapping::Mercator;
    // Past its latitude limit, Mercator clamps the latitude.
    let within_limit = SAMPLES.into_iter().filter(|[_, lat]| lat.abs() <= LATITUDE_MAX);
    for [lng, lat] in within_limit.chain([[-180.0, -85.0], [180.0, 85.0]]) {
        let expected = mercator.world_from_lat_lng(&ll(lat, lng));
        let actual = world.world_from_lat_lng(&ll(lat, lng));
        assert_near(expected.x, actual.x, 1e-12);
        assert_near(expected.y, actual.y, 1e-12);
    }
    for [x, y] in [[0.5, 0.5], [0.1, 0.9], [0.999, 0.001], [0.25, 0.75]] {
        let expected = lat_lng(&mercator, x, y);
        let actual = lat_lng(&world, x, y);
        assert_near(expected.longitude(), actual.longitude(), 5e-11);
        assert_near(expected.latitude(), actual.latitude(), 5e-11);
    }
    let center = Point::new(0.5, 0.5);
    assert_near(mercator.meters_per_world_unit(&center), world.meters_per_world_unit(&center), 5e-7);
}

// gtest: CRSWorldCoordinateHelper.LatitudeRangeOfAPolarProjection
#[test]
fn a_north_polar_projection_stops_its_latitude_range_short_of_the_south_pole() {
    // The opposite pole projects to infinity: the range ends where the
    // meridian stretches ten times as much as at the north pole.
    let world = mapping(crs::north_polar_stereographic("test:polar"));
    assert_eq!([-53.0, 90.0], crs_mapping(&world).latitude_range());
    let world = mapping(crs::polar_stereographic("test:polar"));
    assert_eq!([-53.0, 90.0], crs_mapping(&world).latitude_range());
}

// GL JS: latitudeRange runs from the near pole to where the meridian
// stretches ten times as much as at the center in a south polar CRS.
#[test]
fn a_south_polar_projection_stops_its_latitude_range_short_of_the_north_pole() {
    let Crs { converter: north, .. } = crs::polar_stereographic("test:north");
    let north = Arc::new(north);
    let forward = Arc::clone(&north);
    let world = mapping(crs(
        "test:south-polar",
        move |[lng, lat]| {
            let mut position = [[0.0; 2]];
            forward.forward(&[[lng, -lat]], &mut position);
            position[0]
        },
        move |position| {
            let mut lng_lat = [[0.0; 2]];
            north.inverse(&[position], &mut lng_lat);
            [lng_lat[0][0], -lng_lat[0][1]]
        },
        [-4_194_304.0, 4_194_304.0],
        8_388_608.0,
    ));
    assert_eq!([-90.0, 53.0], crs_mapping(&world).latitude_range());
}

// gtest: CRSWorldCoordinateHelper.LatitudeRangeOfTheIdentity
#[test]
fn the_identity_keeps_every_latitude() {
    assert_eq!([-90.0, 90.0], crs_mapping(&identity()).latitude_range());
}

#[test]
fn a_mercator_box_is_the_rectangle_of_its_corners() {
    let world = WorldMapping::Mercator;
    let bounds = LatLngBounds::hull(ll(-10.0, -20.0), ll(30.0, 40.0));
    let southwest = world.world_from_lat_lng(&bounds.southwest());
    let northeast = world.world_from_lat_lng(&bounds.northeast());
    let world_box = world.lng_lat_box_to_world_box(&bounds);
    assert_eq!(Point::new(southwest.x, northeast.y), world_box.min);
    assert_eq!(Point::new(northeast.x, southwest.y), world_box.max);
}

#[test]
fn a_polar_cap_box_holds_its_parallel_around_the_pole() {
    // The corners of the cap north of 60 N are the pole, at the center of the
    // square, and one point on its edge, a circle around the pole.
    let world = mapping(crs::north_polar_stereographic("test:polar"));
    let radius = world.world_from_lat_lng(&ll(60.0, 0.0)).y - 0.5;
    let world_box = world.lng_lat_box_to_world_box(&LatLngBounds::hull(ll(60.0, -180.0), ll(90.0, 180.0)));
    for (inner, outer) in [(0.5 - radius, world_box.min.x), (0.5 - radius, world_box.min.y)] {
        assert!(outer <= inner && outer >= inner - 0.05 * radius, "{outer} around {inner}");
    }
    for (inner, outer) in [(0.5 + radius, world_box.max.x), (0.5 + radius, world_box.max.y)] {
        assert!(outer >= inner && outer <= inner + 0.05 * radius, "{outer} around {inner}");
    }
}

#[test]
fn mercator_projects_with_its_own_formulas() {
    let world = WorldMapping::Mercator;
    assert!(world.wraps());
    assert_eq!("mercator", world.name());
    let lat_lng = ll(40.75, -73.98);
    assert_eq!(crate::geo::projection::Projection::project(&lat_lng, 3.5), world.project(&lat_lng, 3.5));
    let point = Point::new(1000.0, 700.0);
    assert_eq!(
        crate::geo::projection::Projection::unproject(&point, 3.5, mln_style::geo::WrapMode::Unwrapped).unwrap(),
        world.unproject(&point, 3.5, mln_style::geo::WrapMode::Unwrapped).unwrap()
    );
    let extent = world.tile_matrix_set().extent_at_zoom0;
    assert_near(2.0 * std::f64::consts::PI * EARTH_RADIUS_M, extent, 1e-6);
}

#[test]
fn mappings_are_equal_when_they_are_the_same_one() {
    let a = identity();
    assert_eq!(a, a.clone());
    assert_ne!(a, identity());
    assert_ne!(a, WorldMapping::Mercator);
    assert_eq!(WorldMapping::Mercator, WorldMapping::Mercator);
}

// gtest: HillshadeTweaker.NoLatitudeRangeInAWorldThatDoesNotWrap
#[test]
fn a_world_that_doesnt_wrap_has_no_latitude_range() {
    let id = crate::tile::UnwrappedTileID::new(12, 758, 100);
    assert_eq!([0.0, 0.0], crate::geo::lat_range(&id, &identity()));
    // The same tile row in Mercator is past 84 N, where the shader divides
    // slopes by the cosine of the latitude.
    let mercator = crate::geo::lat_range(&id, &WorldMapping::Mercator);
    assert!(mercator[1] > 84.0, "{mercator:?}");
    assert!(mercator[0] > mercator[1], "{mercator:?}");
}
