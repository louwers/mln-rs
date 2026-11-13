// Ports the fixture-free tests of
// vendor/maplibre-native-base/deps/geojson-vt-cpp/test/test.cpp:
// Simplify.Points, Clip.* and GeoJSONVT.ClipVertexOnTileBorder.
// tests/fixtures.rs has the rest.

use feature::Value;
use geojson_vt::detail::{Clipper, VtGeometry, VtLineString, VtLinearRing, VtPoint, simplify};
use geojson_vt::{GeoJSONVT, Options};
use geometry::{Geometry, Point};

fn p(x: f64, y: f64) -> VtPoint {
    VtPoint::xy(x, y)
}

fn line(points: &[(f64, f64)]) -> VtLineString {
    VtLineString::new(points.iter().map(|&(x, y)| p(x, y)).collect())
}

fn ring(points: &[(f64, f64)]) -> VtLinearRing {
    VtLinearRing::new(points.iter().map(|&(x, y)| p(x, y)).collect())
}

/// `EXPECT_DOUBLE_EQ`: within 4 ULPs.
fn assert_double_eq(a: f64, b: f64) {
    let (ia, ib) = (a.to_bits() as i64, b.to_bits() as i64);
    let to_biased = |i: i64| if i < 0 { i64::MIN - i } else { i };
    assert!(a == b || (to_biased(ia) - to_biased(ib)).abs() <= 4, "{a} != {b}");
}

/// The test's `operator==(vt_point, vt_point)` (`EXPECT_DOUBLE_EQ` on x and
/// y) applied through the geometry variants.
fn assert_geometry_eq(a: &VtGeometry, b: &VtGeometry) {
    fn points(a: &[VtPoint], b: &[VtPoint]) {
        assert_eq!(a.len(), b.len(), "{a:?} != {b:?}");
        for (p, q) in a.iter().zip(b) {
            assert_double_eq(p.x, q.x);
            assert_double_eq(p.y, q.y);
        }
    }
    match (a, b) {
        (VtGeometry::MultiPoint(a), VtGeometry::MultiPoint(b)) => points(a, b),
        (VtGeometry::LineString(a), VtGeometry::LineString(b)) => points(a, b),
        (VtGeometry::MultiLineString(a), VtGeometry::MultiLineString(b)) => {
            assert_eq!(a.len(), b.len());
            a.iter().zip(b).for_each(|(a, b)| points(a, b));
        }
        (VtGeometry::Polygon(a), VtGeometry::Polygon(b)) => {
            assert_eq!(a.len(), b.len());
            a.iter().zip(b).for_each(|(a, b)| points(a, b));
        }
        _ => panic!("{a:?} != {b:?}"),
    }
}

#[test]
fn simplify_points() {
    #[rustfmt::skip]
    let mut points: Vec<VtPoint> = [
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

    let result: Vec<&VtPoint> = points.iter().filter(|p| p.z > 0.005 * 0.005).collect();

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

    let clip = Clipper::<0>::new(10.0, 40.0, false);

    let clipped1 = clip.line_string(&points1);
    let clipped2 = clip.line_string(&points2);

    let expected1 = VtGeometry::MultiLineString(vec![
        line(&[(10.0, 0.0), (40.0, 0.0)]),
        line(&[(40.0, 10.0), (20.0, 10.0), (20.0, 20.0), (30.0, 20.0), (30.0, 30.0), (40.0, 30.0)]),
        line(&[(40.0, 40.0), (25.0, 40.0), (25.0, 50.0), (10.0, 50.0)]),
        line(&[(10.0, 60.0), (25.0, 60.0), (30.0, 60.0)]),
    ]);

    let expected2 =
        VtGeometry::MultiLineString(vec![line(&[(10.0, 0.0), (40.0, 0.0)]), line(&[(40.0, 10.0), (10.0, 10.0)])]);

    assert_geometry_eq(&expected1, &clipped1);
    assert_geometry_eq(&expected2, &clipped2);
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

    let clip = Clipper::<0>::new(10.0, 40.0, true /*lineMetrics*/);

    let VtGeometry::MultiLineString(clipped) = clip.line_string(&points1) else { panic!("not a multi line string") };

    assert_eq!(clipped[0].seg_start, 10.0);
    assert_eq!(clipped[0].seg_end, 40.0);

    assert_eq!(clipped[1].seg_start, 70.0);
    assert_eq!(clipped[1].seg_end, 130.0);

    assert_eq!(clipped[2].seg_start, 160.0);
    assert_eq!(clipped[2].seg_end, 200.0);

    assert_eq!(clipped[3].seg_start, 230.0);
    assert_eq!(clipped[3].seg_end, 245.0);
}

#[test]
fn clip_polygons() {
    let points1 = vec![ring(&[
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
    ])];
    let points2 = vec![ring(&[(0.0, 0.0), (50.0, 0.0), (50.0, 10.0), (0.0, 10.0), (0.0, 0.0)])];

    let clip = Clipper::<0>::new(10.0, 40.0, false);

    let clipped1 = clip.polygon(&points1);
    let clipped2 = clip.polygon(&points2);

    let expected1 = VtGeometry::Polygon(vec![ring(&[
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
    ])]);

    let expected2 =
        VtGeometry::Polygon(vec![ring(&[(10.0, 0.0), (40.0, 0.0), (40.0, 10.0), (10.0, 10.0), (10.0, 0.0)])]);

    assert_geometry_eq(&expected1, &clipped1);
    assert_geometry_eq(&expected2, &clipped2);
}

#[test]
fn clip_points() {
    let pts = |v: &[(f64, f64)]| -> Vec<VtPoint> { v.iter().map(|&(x, y)| p(x, y)).collect() };
    let points1 = pts(&[
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
    let points2 = pts(&[(0.0, 0.0), (50.0, 0.0), (50.0, 10.0), (0.0, 10.0)]);

    let clip = Clipper::<0>::new(10.0, 40.0, false);

    let clipped1 = clip.multi_point(&points1);
    let clipped2 = clip.multi_point(&points2);

    let expected1 = VtGeometry::MultiPoint(pts(&[
        (20.0, 10.0),
        (20.0, 20.0),
        (30.0, 20.0),
        (30.0, 30.0),
        (25.0, 40.0),
        (25.0, 50.0),
        (25.0, 60.0),
    ]));
    let expected2 = VtGeometry::MultiPoint(vec![]);

    assert_geometry_eq(&expected1, &clipped1);
    assert_geometry_eq(&expected2, &clipped2);
}

#[test]
fn geojsonvt_clip_vertex_on_tile_border() {
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
    let geojson = geojson::parse(data).unwrap();
    let mut options = Options::default();
    options.tile.line_metrics = true;
    options.tile.buffer = 2048;
    options.tile.extent = 8192;

    let k_epsilon = 1e-5;

    let mut index = GeoJSONVT::from_geojson(&geojson, options);

    let tile = index.get_tile(13, 2344, 3134).unwrap();
    assert_ne!(tile.features, [] as [feature::Feature<i16>; 0]);
    let expected = vec![Point::new(-2048i16, 2747), Point::new(408, 5037)];
    assert_eq!(tile.features[0].geometry, Geometry::LineString(expected));

    // Check line metrics
    let props = &tile.features[0].properties;
    let Value::Double(clip_start1) = props["mapbox_clip_start"] else { panic!() };
    let Value::Double(clip_end1) = props["mapbox_clip_end"] else { panic!() };
    assert!((0.660_622 - clip_start1).abs() <= k_epsilon);
    assert!((1.0 - clip_end1).abs() <= k_epsilon);
}

/// Twice the signed area of a ring, positive for clockwise with y down.
fn shoelace(ring: &[Point<i16>]) -> i64 {
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
    let mut index = GeoJSONVT::from_geojson(&geojson::parse(data).unwrap(), Options::default());
    let tile = index.get_tile(0, 0, 0).unwrap();
    let Geometry::MultiPolygon(polygons) = &tile.features[0].geometry else { panic!("{:?}", tile.features) };
    assert_eq!(polygons.len(), 2);
    for polygon in polygons {
        assert_eq!(polygon.len(), 2);
        assert!(polygon.iter().all(|ring| ring.first() == ring.last()), "{polygon:?}");
        assert!(shoelace(&polygon[0]) > 0, "outer ring {:?}", polygon[0]);
        assert!(shoelace(&polygon[1]) < 0, "hole {:?}", polygon[1]);
    }
}
