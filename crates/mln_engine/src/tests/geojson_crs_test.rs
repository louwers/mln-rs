//! GeoJSON moved between lng/lat and a registered projection's world square:
//! MapLibre Native's `test/util/geojson_reproject.test.cpp` and GL JS's
//! `src/util/geojson_reproject.test.ts`, over the renderer's mappings.

use std::sync::Arc;

use geo_types::{Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Polygon};
use mln_style::crs::{Crs, CrsMapping, TileMatrixSet};
use mln_style::feature::{Feature, Identifier, PropertyMap, Value};
use mln_style::sources::geojson::{features_from_world, features_to_world};

use crate::tests::crs::{crs, polar_stereographic};

/// The mapping of `crs`, as a registry holds it.
fn mapping(crs: Crs) -> CrsMapping {
    CrsMapping::new(Arc::new(crs))
}

fn identity() -> CrsMapping {
    mapping(Crs::identity())
}

fn coord(x: f64, y: f64) -> Coord {
    Coord { x, y }
}

fn line(positions: &[(f64, f64)]) -> LineString {
    LineString(positions.iter().map(|&(x, y)| coord(x, y)).collect())
}

fn polygon(ring: &[(f64, f64)]) -> Polygon {
    Polygon::new(line(ring), Vec::new())
}

/// `geometry` alone, moved to the world square of `crs`.
fn geometry_to_world(geometry: impl Into<Geometry>, crs: &CrsMapping) -> Option<Geometry> {
    let [feature] = <[Feature; 1]>::try_from(features_to_world(vec![Feature::new(geometry)], crs)).unwrap();
    feature.geometry
}

/// The outer ring of `geometry`, a polygon.
fn ring_of(geometry: Option<Geometry>) -> Vec<Coord> {
    let Some(Geometry::Polygon(polygon)) = geometry else { panic!("not a polygon: {geometry:?}") };
    polygon.exterior().0.clone()
}

/// Whether the segment from `a` to `b` reaches into the world square.
fn segment_intersects_world_square(a: Coord, b: Coord) -> bool {
    let (mut from, mut to) = (0.0, 1.0);
    for (p, q) in [(-(b.x - a.x), a.x), (b.x - a.x, 1.0 - a.x), (-(b.y - a.y), a.y), (b.y - a.y, 1.0 - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            from = f64::max(from, q / p);
        } else {
            to = f64::min(to, q / p);
        }
    }
    from <= to
}

fn line_intersects_world_square(line: &[Coord]) -> bool {
    line.windows(2).any(|segment| segment_intersects_world_square(segment[0], segment[1]))
}

fn ring_contains(ring: &[Coord], point: Coord) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > point.y) != (b.y > point.y) && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn polar() -> CrsMapping {
    mapping(polar_stereographic("test:polar"))
}

// gtest: ReprojectGeoJSONToWorld.MovesAnIdentityPositionToItsWorldCoordinates
#[test]
fn moves_an_identity_position_to_its_world_coordinates() {
    let point = geometry_to_world(geo_types::Point::new(45.0, 45.0), &identity());
    assert_eq!(Some(Geometry::Point(geo_types::Point::new(0.75, 0.25))), point);
}

// gtest: ReprojectGeoJSONToWorld.WalksFeaturesCollectionsAndGeometryCollections
#[test]
fn walks_features_collections_and_geometry_collections() {
    let properties: PropertyMap = [("name".to_owned(), Value::String("a".to_owned()))].into_iter().collect();
    let features: Vec<Feature> = vec![
        Feature {
            geometry: Some(polygon(&[(0.0, 0.0), (45.0, 0.0), (45.0, 45.0), (0.0, 45.0), (0.0, 0.0)]).into()),
            properties: properties.clone(),
            id: Identifier::String("a".to_owned()),
        },
        Feature::new(Geometry::GeometryCollection(GeometryCollection(vec![
            MultiLineString(vec![line(&[(0.0, 0.0), (45.0, 45.0)])]).into(),
            geo_types::Point::new(45.0, 0.0).into(),
        ]))),
        Feature::default(),
    ];

    let result = features_to_world(features, &identity());

    assert_eq!(3, result.len());
    assert_eq!(coord(0.75, 0.25), ring_of(result[0].geometry.clone())[2]);
    assert_eq!(Identifier::String("a".to_owned()), result[0].id);
    assert_eq!(properties, result[0].properties);
    let Some(Geometry::GeometryCollection(collection)) = &result[1].geometry else { panic!("not a collection") };
    assert_eq!(Geometry::Point(geo_types::Point::new(0.75, 0.5)), collection.0[1]);
    assert_eq!(None, result[2].geometry);
}

// gtest: ReprojectGeoJSONToWorld.LeavesOutPositionsWithoutAFinitePlace
#[test]
fn leaves_out_positions_without_a_finite_place() {
    let half_world = mapping(crs(
        "test:half",
        |[lng, lat]| if lng > 0.0 { [f64::NAN, lat] } else { [lng, lat] },
        |position| position,
        [-90.0, 90.0],
        180.0,
    ));
    let features = vec![
        Feature::new(geo_types::Point::new(45.0, 0.0)),
        Feature::new(MultiPoint::from(vec![(-45.0, 0.0), (45.0, 0.0)])),
        Feature::new(line(&[(-45.0, 0.0), (45.0, 0.0), (-45.0, 45.0)])),
    ];

    let result = features_to_world(features, &half_world);

    assert_eq!(None, result[0].geometry);
    assert_eq!(Some(MultiPoint::from(vec![(0.25, 0.5)]).into()), result[1].geometry);
    assert_eq!(Some(line(&[(0.25, 0.5), (0.25, 0.25)]).into()), result[2].geometry);
}

// gtest: ReprojectGeoJSONToWorld.KeepsAPolygonWithAnEdgeAcrossTheFarPoleOutOfTheWorldSquare
#[test]
fn keeps_a_polygon_with_an_edge_across_the_far_pole_out_of_the_world_square() {
    let isea4h_south_pole_cell = polygon(&[
        (88.3853, -87.67805),
        (54.96953, -88.57819),
        (0.0, -89.52868),
        (180.0, -89.31865),
        (142.1755, -88.10424),
        (115.34565, -87.43262),
        (88.3853, -87.67805),
    ]);

    let ring = ring_of(geometry_to_world(isea4h_south_pole_cell, &polar()));

    assert!(!line_intersects_world_square(&ring));
    assert!(!ring_contains(&ring, coord(0.5, 0.5)));
}

// gtest: ReprojectGeoJSONToWorld.KeepsALineAlongAParallelPastTheSouthernLimitOutOfTheWorldSquare
#[test]
fn keeps_a_line_along_a_parallel_past_the_southern_limit_out_of_the_world_square() {
    let Some(Geometry::LineString(line)) = geometry_to_world(line(&[(0.0, -80.0), (180.0, -80.0)]), &polar()) else {
        panic!("not a line");
    };
    assert!(!line_intersects_world_square(&line.0));
}

// Past a turn, an edge along the southern limit traces the same parallel
// again: however far it reaches, it gets the positions of a turn at most.
#[test]
fn an_edge_along_the_southern_limit_gets_the_positions_of_a_turn_at_most() {
    let along_60_s_to = |east: f64| {
        let Some(Geometry::LineString(line)) = geometry_to_world(line(&[(0.0, -60.0), (east, -60.0)]), &polar()) else {
            panic!("not a line");
        };
        line.0
    };
    for east in [3600.0, 1e12] {
        let line = along_60_s_to(east);
        assert!(line.len() <= 37, "to {east} E: {} positions", line.len());
        assert!(!line_intersects_world_square(&line), "to {east} E");
    }
}

// gtest: ReprojectGeoJSONToWorld.FillsAnEdgeAlongTheSouthernLimitSoAPolygonOfTheWholeGlobeCoversTheWorldSquare
#[test]
fn fills_an_edge_along_the_southern_limit_so_a_polygon_of_the_whole_globe_covers_the_world_square() {
    let whole_globe = polygon(&[(-180.0, -90.0), (180.0, -90.0), (180.0, 90.0), (-180.0, 90.0), (-180.0, -90.0)]);

    let ring = ring_of(geometry_to_world(whole_globe, &polar()));

    assert!(ring_contains(&ring, coord(0.75, 0.75)));
}

// gtest: ReprojectGeoJSONToWorld.KeepsAPolygonThatCoversASmallGridOverAllOfIt
#[test]
fn keeps_a_polygon_that_covers_a_small_grid_over_all_of_it() {
    let mut grid = polar_stereographic("test:grid");
    let mut center = [[0.0; 2]];
    grid.converter.forward(&[[-5.0, 70.0]], &mut center);
    let [[x, y]] = center;
    grid.tile_matrix_set = TileMatrixSet { origin: [x - 500.0, y + 500.0], extent_at_zoom0: 1000.0 };
    let rectangle = polygon(&[(-20.0, 60.0), (10.0, 60.0), (10.0, 80.0), (-20.0, 80.0), (-20.0, 60.0)]);

    let ring = ring_of(geometry_to_world(rectangle, &mapping(grid)));

    for (x, y) in [(0.5, 0.5), (0.01, 0.01), (0.99, 0.01), (0.99, 0.99), (0.01, 0.99)] {
        assert!(ring_contains(&ring, coord(x, y)), "{x}, {y}");
    }
}

// gtest: ReprojectFeaturesFromWorld.RoundTripsWithReprojectGeoJSONToWorld
#[test]
fn round_trips_with_features_to_world() {
    let features = vec![Feature::new(Geometry::GeometryCollection(GeometryCollection(vec![
        geo_types::Point::new(-30.0, -30.0).into(),
        MultiPolygon(vec![polygon(&[(0.0, 0.0), (45.0, 0.0), (45.0, 45.0), (0.0, 0.0)])]).into(),
    ])))];
    let world = identity();

    let round_tripped = features_from_world(features_to_world(features, &world), &world);

    let Some(Geometry::GeometryCollection(geometries)) = &round_tripped[0].geometry else { panic!("a collection") };
    let Geometry::Point(point) = geometries.0[0] else { panic!("a point") };
    assert!((point.x() + 30.0).abs() < 1e-9 && (point.y() + 30.0).abs() < 1e-9, "{point:?}");
    let Geometry::MultiPolygon(polygons) = &geometries.0[1] else { panic!("a multipolygon") };
    let expected = [(0.0, 0.0), (45.0, 0.0), (45.0, 45.0), (0.0, 0.0)];
    let ring = &polygons.0[0].exterior().0;
    assert_eq!(expected.len(), ring.len());
    for (position, (x, y)) in ring.iter().zip(expected) {
        assert!((position.x - x).abs() < 1e-9 && (position.y - y).abs() < 1e-9, "{position:?}");
    }
}
