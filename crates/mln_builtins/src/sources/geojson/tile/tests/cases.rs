//! Simplification, clipping and cutting, without fixtures.

use geo_types::{Coord, Geometry, LineString, MultiPoint, Point};
use mln_style::feature::{Feature, Value};

use crate::sources::geojson::Coordinates;
use crate::sources::geojson::tile::clip::{clip_line, clip_ring};
use crate::sources::geojson::tile::geometry::{Line, Ring, Vertex, X};
use crate::sources::geojson::tile::simplify::simplify;
use crate::sources::geojson::tile::{Error, Options, TileIndex, TileOptions, cut_tile};

fn p(x: f64, y: f64) -> Vertex {
    Vertex::new(x, y, 0.0)
}

fn line(points: &[(f64, f64)]) -> Line {
    Line { points: points.iter().map(|&(x, y)| p(x, y)).collect(), ..Line::default() }
}

fn ring(points: &[(f64, f64)]) -> Ring {
    Ring { points: points.iter().map(|&(x, y)| p(x, y)).collect(), area: 0.0 }
}

/// Within 4 ULPs.
fn assert_double_eq(a: f64, b: f64) {
    let (ia, ib) = (a.to_bits() as i64, b.to_bits() as i64);
    let to_biased = |i: i64| if i < 0 { i64::MIN - i } else { i };
    assert!(a == b || (to_biased(ia) - to_biased(ib)).abs() <= 4, "{a} != {b}");
}

fn assert_points_eq(a: &[Vertex], b: &[Vertex]) {
    assert_eq!(a.len(), b.len(), "{a:?} != {b:?}");
    for (p, q) in a.iter().zip(b) {
        assert_double_eq(p.x, q.x);
        assert_double_eq(p.y, q.y);
    }
}

fn assert_lines_eq(a: &[Line], b: &[Line]) {
    assert_eq!(a.len(), b.len(), "{a:?} != {b:?}");
    a.iter().zip(b).for_each(|(a, b)| assert_points_eq(&a.points, &b.points));
}

fn clip_lines(input: &Line, line_metrics: bool) -> Vec<Line> {
    let mut pieces = crate::sources::geojson::tile::geometry::Parts::new();
    clip_line::<X>(input, 10.0, 40.0, line_metrics, &mut pieces);
    pieces.into_vec()
}

#[test]
fn simplify_points() {
    #[rustfmt::skip]
    let mut points: Vec<Vertex> = [
        (0.22455, 0.25015), (0.22691, 0.24419), (0.23331, 0.24145), (0.23498, 0.23606),
        (0.24421, 0.23276), (0.26259, 0.21531), (0.26776, 0.21381), (0.27357, 0.20184),
        (0.27312, 0.19216), (0.27762, 0.18903), (0.28036, 0.18141), (0.28651, 0.17774),
        (0.29241, 0.15937), (0.29691, 0.15564), (0.31495, 0.15137), (0.31975, 0.14516),
        (0.33033, 0.13757), (0.34148, 0.13996), (0.36998, 0.13789), (0.38739, 0.14251),
        (0.39128, 0.13939), (0.40952, 0.14114), (0.41482, 0.13975), (0.42772, 0.12730),
        (0.43960, 0.11974), (0.47493, 0.10787), (0.48651, 0.10675), (0.48920, 0.10945),
        (0.49379, 0.10863), (0.50474, 0.11966), (0.51296, 0.12235), (0.51863, 0.12089),
        (0.52409, 0.12688), (0.52957, 0.12786), (0.53421, 0.14093), (0.53927, 0.14724),
        (0.56769, 0.14891), (0.57525, 0.15726), (0.58062, 0.15815), (0.60153, 0.15685),
        (0.61774, 0.15986), (0.62200, 0.16704), (0.62955, 0.19460), (0.63890, 0.19561),
        (0.64126, 0.20081), (0.65177, 0.20456), (0.67155, 0.22255), (0.68368, 0.21745),
        (0.69525, 0.21915), (0.70064, 0.21798), (0.70312, 0.21436), (0.71226, 0.21587),
        (0.72149, 0.21281), (0.72781, 0.21336), (0.72998, 0.20873), (0.73532, 0.20820),
        (0.73994, 0.20477), (0.76998, 0.20842), (0.77960, 0.21687), (0.78420, 0.21816),
        (0.80024, 0.21462), (0.81053, 0.21973), (0.81719, 0.22682), (0.82077, 0.23617),
        (0.82723, 0.23616), (0.82989, 0.23989), (0.85100, 0.24894), (0.85988, 0.25549),
        (0.86521, 0.26853), (0.85795, 0.28030), (0.86548, 0.29145), (0.86681, 0.29866),
        (0.86468, 0.30271), (0.86779, 0.30617), (0.85987, 0.31137), (0.86008, 0.31435),
        (0.85829, 0.31494), (0.85810, 0.32760), (0.85454, 0.33540), (0.86092, 0.34300),
        (0.85643, 0.35015), (0.85142, 0.35296), (0.84984, 0.35959), (0.85456, 0.36553),
        (0.84974, 0.37038), (0.84409, 0.37189), (0.84475, 0.38044), (0.84152, 0.38367),
        (0.83957, 0.39040), (0.84559, 0.39905), (0.84840, 0.40755), (0.84371, 0.41130),
        (0.84409, 0.41988), (0.83951, 0.43276), (0.84133, 0.44104), (0.84762, 0.44922),
        (0.84716, 0.45844), (0.85138, 0.46279), (0.85397, 0.47115), (0.86636, 0.48077),
    ]
    .iter()
    .map(|&(x, y)| p(x, y))
    .collect();

    #[rustfmt::skip]
    let simplified: Vec<(f64, f64)> = vec![
        (0.22455, 0.25015), (0.26776, 0.21381), (0.29691, 0.15564), (0.33033, 0.13757),
        (0.40952, 0.14114), (0.4396, 0.11974),  (0.48651, 0.10675), (0.52957, 0.12786),
        (0.53927, 0.14724), (0.56769, 0.14891), (0.61774, 0.15986), (0.62955, 0.1946),
        (0.67155, 0.22255), (0.72781, 0.21336), (0.73994, 0.20477), (0.76998, 0.20842),
        (0.7842, 0.21816),  (0.80024, 0.21462), (0.82077, 0.23617), (0.85988, 0.25549),
        (0.86521, 0.26853), (0.85795, 0.2803),  (0.86779, 0.30617), (0.85829, 0.31494),
        (0.85454, 0.3354),  (0.86092, 0.343),   (0.84984, 0.35959), (0.85456, 0.36553),
        (0.84409, 0.37189), (0.83957, 0.3904),  (0.8484, 0.40755),  (0.83951, 0.43276),
        (0.85397, 0.47115), (0.86636, 0.48077),
    ];

    simplify(&mut points, 0.001);

    let result: Vec<&Vertex> = points.iter().filter(|p| p.importance > 0.005 * 0.005).collect();

    assert_eq!(result.len(), simplified.len());
    for (a, b) in result.iter().zip(&simplified) {
        assert_double_eq(a.x, b.0);
        assert_double_eq(a.y, b.1);
    }
}

#[test]
fn clip_polylines() {
    let points1 = line(&[
        (0.0, 0.0),
        (50.0, 0.0),
        (50.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (30.0, 20.0),
        (30.0, 30.0),
        (50.0, 30.0),
        (50.0, 40.0),
        (25.0, 40.0),
        (25.0, 50.0),
        (0.0, 50.0),
        (0.0, 60.0),
        (25.0, 60.0),
        (30.0, 60.0),
    ]);
    let points2 = line(&[(0.0, 0.0), (50.0, 0.0), (50.0, 10.0), (0.0, 10.0)]);

    let clipped1 = clip_lines(&points1, false);
    let clipped2 = clip_lines(&points2, false);

    let expected1 = [
        line(&[(10.0, 0.0), (40.0, 0.0)]),
        line(&[(40.0, 10.0), (20.0, 10.0), (20.0, 20.0), (30.0, 20.0), (30.0, 30.0), (40.0, 30.0)]),
        line(&[(40.0, 40.0), (25.0, 40.0), (25.0, 50.0), (10.0, 50.0)]),
        line(&[(10.0, 60.0), (25.0, 60.0), (30.0, 60.0)]),
    ];

    let expected2 = [line(&[(10.0, 0.0), (40.0, 0.0)]), line(&[(40.0, 10.0), (10.0, 10.0)])];

    assert_lines_eq(&expected1, &clipped1);
    assert_lines_eq(&expected2, &clipped2);
}

#[test]
fn clip_polylines_line_metrics() {
    let points1 = line(&[
        (0.0, 0.0),
        (50.0, 0.0),
        (50.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (30.0, 20.0),
        (30.0, 30.0),
        (50.0, 30.0),
        (50.0, 40.0),
        (25.0, 40.0),
        (25.0, 50.0),
        (0.0, 50.0),
        (0.0, 60.0),
        (25.0, 60.0),
    ]);

    let clipped = clip_lines(&points1, true);

    assert_eq!(clipped[0].start, 10.0);
    assert_eq!(clipped[0].end, 40.0);

    assert_eq!(clipped[1].start, 70.0);
    assert_eq!(clipped[1].end, 130.0);

    assert_eq!(clipped[2].start, 160.0);
    assert_eq!(clipped[2].end, 200.0);

    assert_eq!(clipped[3].start, 230.0);
    assert_eq!(clipped[3].end, 245.0);
}

#[test]
fn clip_polygons() {
    let points1 = ring(&[
        (0.0, 0.0),
        (50.0, 0.0),
        (50.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (30.0, 20.0),
        (30.0, 30.0),
        (50.0, 30.0),
        (50.0, 40.0),
        (25.0, 40.0),
        (25.0, 50.0),
        (0.0, 50.0),
        (0.0, 60.0),
        (25.0, 60.0),
        (0.0, 0.0),
    ]);
    let points2 = ring(&[(0.0, 0.0), (50.0, 0.0), (50.0, 10.0), (0.0, 10.0), (0.0, 0.0)]);

    let clipped1 = clip_ring::<X>(&points1, 10.0, 40.0);
    let clipped2 = clip_ring::<X>(&points2, 10.0, 40.0);

    let expected1 = ring(&[
        (10.0, 0.0),
        (40.0, 0.0),
        (40.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (30.0, 20.0),
        (30.0, 30.0),
        (40.0, 30.0),
        (40.0, 40.0),
        (25.0, 40.0),
        (25.0, 50.0),
        (10.0, 50.0),
        (10.0, 60.0),
        (25.0, 60.0),
        (10.0, 24.0),
        (10.0, 0.0),
    ]);

    let expected2 = ring(&[(10.0, 0.0), (40.0, 0.0), (40.0, 10.0), (10.0, 10.0), (10.0, 0.0)]);

    assert_points_eq(&expected1.points, &clipped1.points);
    assert_points_eq(&expected2.points, &clipped2.points);
}

#[test]
fn clip_vertex_on_tile_border() {
    let data = r#"{
        "type": "Feature",
        "geometry": {
            "type": "LineString",
            "coordinates":[
                [-77.031373697916663,38.895516493055553],
                [-77.01416015625,38.887532552083336],
                [-76.99,38.87]
            ]
        }
    }"#; // The second node is exactly on the (13, 2344, 3134) tile border.
    let geojson = mln_style::feature::geojson::features(data.parse().unwrap()).unwrap();
    let mut options = Options::default();
    options.tile.line_metrics = true;
    options.tile.buffer = 2048;
    options.tile.extent = 8192;

    let k_epsilon = 1e-5;

    let index = TileIndex::new(&geojson, options);

    let tile = index.tile(13, 2344, 3134).unwrap();
    assert!(!tile.is_empty());
    let expected = LineString::from(vec![(-2048i16, 2747), (408, 5037)]);
    assert_eq!(tile[0].geometry, Some(Geometry::LineString(expected)));

    // Check line metrics
    let props = &tile[0].properties;
    let Value::Double(clip_start1) = props["mapbox_clip_start"] else { panic!() };
    let Value::Double(clip_end1) = props["mapbox_clip_end"] else { panic!() };
    assert!((0.660_622 - clip_start1).abs() <= k_epsilon);
    assert!((1.0 - clip_end1).abs() <= k_epsilon);
}

/// Twice the signed area of a ring, positive for clockwise with y down.
fn shoelace(ring: &[Coord<i16>]) -> i64 {
    let mut sum = 0;
    for (p, q) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        sum += i64::from(p.x) * i64::from(q.y) - i64::from(q.x) * i64::from(p.y);
    }
    sum
}

#[test]
fn polygon_rings_are_closed_and_rewound() {
    // The first polygon is wound as RFC 7946 wants, the second the other
    // way round, both with a hole. The second one's outer ring is open.
    let data = r#"{
        "type": "MultiPolygon",
        "coordinates": [
            [[[0, 0], [40, 0], [40, 40], [0, 40], [0, 0]], [[10, 10], [10, 30], [30, 30], [30, 10], [10, 10]]],
            [[[-40, 0], [-40, 40], [-80, 40], [-80, 0]], [[-50, 10], [-70, 10], [-70, 30], [-50, 30], [-50, 10]]]
        ]
    }"#;
    let index =
        TileIndex::new(&mln_style::feature::geojson::features(data.parse().unwrap()).unwrap(), Options::default());
    let tile = index.tile(0, 0, 0).unwrap();
    let Some(Geometry::MultiPolygon(polygons)) = &tile[0].geometry else { panic!("{tile:?}") };
    assert_eq!(polygons.0.len(), 2);
    for polygon in polygons {
        let [hole] = polygon.interiors() else { panic!("{polygon:?}") };
        assert!([polygon.exterior(), hole].iter().all(|ring| ring.0.first() == ring.0.last()), "{polygon:?}");
        assert!(shoelace(&polygon.exterior().0) > 0, "outer ring {:?}", polygon.exterior());
        assert!(shoelace(&hole.0) < 0, "hole {hole:?}");
    }
}

#[test]
fn a_tile_past_the_max_zoom_or_the_last_row_is_an_error() {
    let geojson =
        mln_style::feature::geojson::features(r#"{"type": "Point", "coordinates": [0, 0]}"#.parse().unwrap()).unwrap();
    let index = TileIndex::new(&geojson, Options { max_zoom: 4, ..Options::default() });
    assert_eq!(index.tile(5, 0, 0), Err(Error::ZoomPastMax(5)));
    assert_eq!(index.tile(2, 0, 4), Err(Error::RowPastEnd { z: 2, y: 4 }));
    // Columns wrap around the world,
    // and share the tile.
    let tile = index.tile(1, 1, 1).unwrap();
    assert!(std::sync::Arc::ptr_eq(&index.tile(1, 3, 1).unwrap(), &tile));
}

/// The options of an index of features in the unit square.
fn unit_square() -> Options {
    Options {
        tile: TileOptions { coordinates: Coordinates::UnitSquare, ..TileOptions::default() },
        ..Options::default()
    }
}

/// An index of one feature, of `geometry`.
fn index_of(geometry: impl Into<Geometry>, options: Options) -> TileIndex {
    TileIndex::new(&[Feature::new(geometry)], options)
}

#[test]
fn unit_square_without_wrap_cuts_at_the_buffer_around_the_world() {
    let from_three_worlds_north_to_east = LineString::from(vec![(0.25, -3.0), (0.75, 0.5), (1.25, 0.5)]);
    let index = index_of(from_three_worlds_north_to_east, Options { wrap: false, ..unit_square() });

    let tile = index.tile(0, 0, 0).unwrap();
    let [feature] = &tile[..] else { panic!("{tile:?}") };
    let expected = LineString::from(vec![(2770i16, -64), (3072, 2048), (4160, 2048)]);
    assert_eq!(feature.geometry, Some(Geometry::LineString(expected)));
}

#[test]
fn unit_square_wraps_around_the_antimeridian_by_default() {
    let index = index_of(Point::new(0.999, 0.25), unit_square());

    let tile = index.tile(1, 0, 0).unwrap();
    let [feature] = &tile[..] else { panic!("{tile:?}") };
    assert_eq!(feature.geometry, Some(Geometry::Point(Point::new(-8, 2048))));
}

#[test]
fn unit_square_that_wraps_is_cut_above_and_below() {
    let from_three_above_to_three_below = LineString::from(vec![(0.25, -3.0), (0.75, 3.0)]);
    let index = index_of(from_three_above_to_three_below, unit_square());

    let tile = index.tile(0, 0, 0).unwrap();
    let [feature] = &tile[..] else { panic!("{tile:?}") };
    let expected = LineString::from(vec![(2043i16, -64), (2395, 4160)]);
    assert_eq!(feature.geometry, Some(Geometry::LineString(expected)));
}

#[test]
fn lng_lat_without_wrap_copies_nothing_across_the_antimeridian() {
    let index = index_of(Point::new(179.9, -10.0), Options { wrap: false, ..Options::default() });

    assert!(index.tile(1, 0, 1).unwrap().is_empty());
    assert_eq!(index.tile(1, 1, 1).unwrap().len(), 1);
}

#[test]
fn unit_square_without_wrap_has_no_tile_beside_the_world() {
    let in_the_two_top_tiles = MultiPoint::from(vec![(0.25, 0.25), (0.75, 0.25)]);
    let index = index_of(in_the_two_top_tiles, Options { wrap: false, ..unit_square() });

    assert_eq!(index.tile(1, 0, 0).unwrap().len(), 1);
    assert!(index.tile(1, 2, 0).unwrap().is_empty());
    assert!(index.tile(1, u32::MAX, 0).unwrap().is_empty());
}

#[test]
fn unit_square_leaves_out_points_outside_the_world() {
    let below_the_world = Point::new(0.5, 5.0);
    let at_infinity = Point::new(0.5, f64::INFINITY);
    let in_the_world = Point::new(0.5, 0.5);
    let features = [below_the_world, at_infinity, in_the_world].map(Feature::new);
    let index = TileIndex::new(&features, Options { wrap: false, ..unit_square() });

    let tile = index.tile(0, 0, 0).unwrap();
    let [feature] = &tile[..] else { panic!("{tile:?}") };
    assert_eq!(feature.geometry, Some(Geometry::Point(Point::new(2048, 2048))));
}

#[test]
fn cut_tile_unit_square() {
    let options = TileOptions { coordinates: Coordinates::UnitSquare, ..TileOptions::default() };

    let tile = cut_tile(&[Feature::new(Point::new(0.75, 0.25))], 1, 1, 0, &options, false, false);
    let [feature] = &tile[..] else { panic!("{tile:?}") };
    assert_eq!(feature.geometry, Some(Geometry::Point(Point::new(2048, 2048))));
}
