// Ports test/util/tile_cover.test.cpp.
//
// The gtest groups `TileCover`, `TileCoverStream` and `TileCount` are the
// modules `tile_cover`, `tile_cover_stream` and `tile_count`.

use std::rc::Rc;

use geometry::{Geometry, LineString, LinearRing, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon};
use mln_map::transform::{NullTransformObserver, Transform};
use mln_map_types::{CameraOptions, ConstrainMode, TileLodMode, ViewportMode};
use mln_renderer::util::tile_cover::{
    TileCover, TileCoverParameters, covering_zoom_level, tile_count_bounds, tile_cover, tile_cover_bounds,
    tile_cover_geometry,
};
use mln_style_types::{ProjectionDefinition, SourceType};
use mln_tile_id::{CanonicalTileID, OverscaledTileID, UnwrappedTileID};
use mln_util::constants;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, WrapMode};
use mln_util::range::Range;
use mln_util::size::Size;

const ZOOM_RANGE: Range<u8> = Range::new(0, 14);

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap()
}

/// `UnwrappedTileID{z, x, y}`.
fn uw(z: u8, x: i64, y: i64) -> UnwrappedTileID {
    UnwrappedTileID::new(z, x, y)
}

/// `OverscaledTileID{z, x, y}`.
fn os(z: u8, x: u32, y: u32) -> OverscaledTileID {
    OverscaledTileID::from_zxy(z, x, y)
}

/// `OverscaledTileID{overscaledZ, wrap, {z, x, y}}`.
fn osw(overscaled_z: u8, wrap: i16, z: u8, x: u32, y: u32) -> OverscaledTileID {
    OverscaledTileID::new(overscaled_z, wrap, CanonicalTileID::new(z, x, y))
}

/// `CanonicalTileID{z, x, y}`.
fn cn(z: u8, x: u32, y: u32) -> CanonicalTileID {
    CanonicalTileID::new(z, x, y)
}

fn p(x: f64, y: f64) -> Point<f64> {
    Point::new(x, y)
}

fn points(coords: &[(f64, f64)]) -> Vec<Point<f64>> {
    coords.iter().map(|&(x, y)| p(x, y)).collect()
}

/// `Transform transform;`
fn new_transform() -> Transform {
    Transform::new(Rc::new(NullTransformObserver), ConstrainMode::HeightOnly, ViewportMode::Default)
}

/// `Transform transform; transform.resize({width, height});`
fn transform(width: u32, height: u32) -> Transform {
    let mut transform = new_transform();
    transform.resize(Size::new(width, height)).unwrap();
    transform
}

/// `{transform.getState()}`.
fn params(transform: &Transform) -> TileCoverParameters {
    TileCoverParameters::new(transform.get_state().clone())
}

/// gtest's `ASSERT_NEAR(val1, val2, abs_error)`.
fn assert_near(val1: f64, val2: f64, abs_error: f64) {
    assert!((val1 - val2).abs() <= abs_error, "|{val1} - {val2}| > {abs_error}");
}

fn san_francisco() -> LatLngBounds {
    LatLngBounds::hull(ll(37.6609, -122.5744), ll(37.8271, -122.3204))
}

fn san_francisco_wrapped() -> LatLngBounds {
    LatLngBounds::hull(ll(37.6609, 238.5744), ll(37.8271, 238.3204))
}

/// `std::rand()` stand-in for the fuzz tests: a fixed-seed LCG (the C
/// standard's sample `rand`), so runs are reproducible. C++ seeds libc
/// `rand` with `time(nullptr)`.
struct Lcg(u32);

impl Lcg {
    fn rand(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(1103515245).wrapping_add(12345);
        ((self.0 >> 16) & 0x7fff) as usize
    }
}

mod tile_cover {
    use super::*;

    // gtest: TileCover.Empty
    #[test]
    fn empty() {
        assert_eq!(Vec::<UnwrappedTileID>::new(), tile_cover_bounds(&LatLngBounds::empty(), 0));
    }

    // gtest: TileCover.Arctic
    #[test]
    fn arctic() {
        assert_eq!(
            Vec::<UnwrappedTileID>::new(),
            tile_cover_bounds(&LatLngBounds::hull(ll(86.0, -180.0), ll(90.0, 180.0)), 0)
        );
    }

    // gtest: TileCover.Antarctic
    #[test]
    fn antarctic() {
        assert_eq!(
            Vec::<UnwrappedTileID>::new(),
            tile_cover_bounds(&LatLngBounds::hull(ll(-86.0, -180.0), ll(-90.0, 180.0)), 0)
        );
    }

    // gtest: TileCover.WorldZ0
    #[test]
    fn world_z0() {
        assert_eq!(vec![uw(0, 0, 0)], tile_cover_bounds(&LatLngBounds::world(), 0));
    }

    // gtest: TileCover.Pitch
    #[test]
    fn pitch() {
        let mut transform = transform(512, 512);
        // slightly offset center so that tile order is better defined

        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(0.1, -0.1)))
                .with_zoom(Some(2.0))
                .with_bearing(Some(5.0))
                .with_pitch(Some(40.0)),
        );

        assert_eq!(
            vec![os(2, 1, 1), os(2, 2, 1), os(2, 1, 2), os(2, 2, 2)],
            tile_cover(&params(&transform), 2, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(vec![os(2, 1, 2), os(2, 2, 2), os(2, 1, 1), os(2, 2, 1)], tile_cover(&params, 2, ZOOM_RANGE, None));
    }

    // gtest: TileCover.PitchIssue15442
    #[test]
    fn pitch_issue15442() {
        let mut transform = transform(412, 691);

        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(59.116898740996106, 91.565660781803615)))
                .with_zoom(Some(2.0551126748417214))
                .with_bearing(Some(mln_math::rad2deg(0.74963938256567264)))
                .with_pitch(Some(mln_math::rad2deg(1.0471975511965976))),
        );

        assert_eq!(
            vec![os(2, 3, 1), os(2, 2, 1), os(2, 3, 0), os(2, 2, 0), osw(2, 1, 2, 0, 0), osw(2, 1, 2, 1, 0)],
            tile_cover(&params(&transform), 2, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![os(2, 2, 1), os(2, 3, 1), os(2, 2, 0), os(2, 3, 0), osw(2, 1, 2, 0, 0), osw(2, 1, 2, 1, 0)],
            tile_cover(&params, 2, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.HighPitch
    #[test]
    fn high_pitch() {
        let mut transform = transform(512, 512);
        // slightly offset center so that tile order is better defined

        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(0.1, -0.1)))
                .with_zoom(Some(2.0))
                .with_bearing(Some(5.0))
                .with_pitch(Some(60.0)),
        );

        let mut params = params(&transform);
        params.tile_lod_pitch_threshold = 0.0;
        params.tile_lod_min_radius = 1.0;
        assert_eq!(
            vec![os(2, 1, 1), os(2, 2, 1), os(2, 1, 2), os(2, 2, 2), os(2, 1, 0), os(2, 2, 0), os(2, 3, 0)],
            tile_cover(&params, 2, ZOOM_RANGE, None)
        );
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![
                os(2, 1, 1),
                os(2, 2, 1),
                os(3, 3, 4),
                os(3, 3, 5),
                os(2, 1, 0),
                os(2, 2, 0),
                os(3, 4, 4),
                os(2, 3, 0),
                os(3, 4, 5),
                os(3, 5, 4),
            ],
            tile_cover(&params, 2, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.PitchOverAllowedByContentInsets
    #[test]
    fn pitch_over_allowed_by_content_insets() {
        let mut transform = transform(512, 512);

        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(0.1, -0.1)))
                .with_padding(Some(EdgeInsets::new(376.0, 0.0, 0.0, 0.0).unwrap()))
                .with_zoom(Some(8.0))
                .with_bearing(Some(45.0))
                .with_pitch(Some(60.0)),
        );
        // Top padding of 376 leads to capped pitch. See Transform::getMaxPitchForEdgeInsets.
        assert!(transform.get_pitch() + 0.001 <= mln_math::deg2rad(60.0));

        assert_eq!(
            vec![os(3, 4, 3), os(3, 3, 3), os(3, 4, 4), os(3, 3, 4)],
            tile_cover(&params(&transform), 3, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(vec![os(3, 3, 4), os(3, 3, 3), os(3, 4, 4), os(3, 4, 3)], tile_cover(&params, 3, ZOOM_RANGE, None));
    }

    // gtest: TileCover.PitchWithLargerResultSet
    #[test]
    fn pitch_with_larger_result_set() {
        let mut transform = transform(1024, 768);

        // The values used here triggered the regression with left and right edge
        // selection in tile_cover.cpp scanSpans.
        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(0.1, -0.1)))
                .with_padding(Some(EdgeInsets::new(400.0, 0.0, 0.0, 0.0).unwrap()))
                .with_zoom(Some(5.0))
                .with_bearing(Some(-142.2630000003529176))
                .with_pitch(Some(60.0)),
        );

        let cover = tile_cover(&params(&transform), 5, ZOOM_RANGE, None);
        // Returned vector has above 100 elements, we check first 16 as there is a
        // plan to return lower LOD for distant tiles.
        assert_eq!(
            vec![
                os(5, 15, 16),
                os(5, 15, 17),
                os(5, 14, 16),
                os(5, 16, 16),
                os(5, 15, 15),
                os(5, 14, 17),
                os(5, 16, 17),
                os(5, 14, 15),
                os(5, 16, 15),
                os(5, 15, 18),
                os(5, 13, 16),
                os(5, 14, 18),
                os(5, 16, 18),
                os(5, 13, 17),
                os(5, 13, 18),
                os(5, 15, 19),
            ],
            cover[..16].to_vec()
        );
    }

    // gtest: TileCover.WorldZ1
    #[test]
    fn world_z1() {
        assert_eq!(
            vec![uw(1, 0, 0), uw(1, 0, 1), uw(1, 1, 0), uw(1, 1, 1)],
            tile_cover_bounds(&LatLngBounds::world(), 1)
        );
    }

    // gtest: TileCover.SingletonZ0
    #[test]
    fn singleton_z0() {
        assert_eq!(Vec::<UnwrappedTileID>::new(), tile_cover_bounds(&LatLngBounds::singleton(ll(0.0, 0.0)), 0));
    }

    // gtest: TileCover.SingletonZ1
    #[test]
    fn singleton_z1() {
        assert_eq!(Vec::<UnwrappedTileID>::new(), tile_cover_bounds(&LatLngBounds::singleton(ll(0.0, 0.0)), 1));
    }

    // gtest: TileCover.CoordinatesAreUnwrapped
    #[test]
    fn coordinates_are_unwrapped() {
        let mut transform = transform(512, 512);
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.1, -180.1))).with_zoom(Some(1.0)));

        assert_eq!(
            vec![osw(1, 0, 1, 1, 0), osw(1, 1, 1, 0, 0), osw(1, 0, 1, 1, 1), osw(1, 1, 1, 0, 1)],
            tile_cover(&params(&transform), 1, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![osw(1, 0, 1, 1, 0), osw(1, 1, 1, 0, 0), osw(1, 0, 1, 1, 1), osw(1, 1, 1, 0, 1)],
            tile_cover(&params, 1, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.DifferentOverscaledZ
    #[test]
    fn different_overscaled_z() {
        let mut transform = transform(512, 512);
        // slightly offset center so that tile order is better defined

        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.1, -0.1))).with_zoom(Some(2.0)));

        assert_eq!(
            vec![osw(3, 0, 2, 1, 1), osw(3, 0, 2, 2, 1), osw(3, 0, 2, 1, 2), osw(3, 0, 2, 2, 2)],
            tile_cover(&params(&transform), 2, ZOOM_RANGE, Some(3))
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![osw(3, 0, 2, 1, 1), osw(3, 0, 2, 2, 1), osw(3, 0, 2, 1, 2), osw(3, 0, 2, 2, 2)],
            tile_cover(&params, 2, ZOOM_RANGE, Some(3))
        );
    }

    // gtest: TileCover.DifferentOverscaledZWithPitch
    #[test]
    fn different_overscaled_z_with_pitch() {
        let mut transform = transform(512, 512);
        transform.jump_to(
            &CameraOptions::default().with_center(Some(ll(30.0, -10.0))).with_zoom(Some(3.5)).with_pitch(Some(61.0)),
        );

        let expected = vec![
            osw(5, 0, 3, 3, 3),
            osw(5, 0, 3, 4, 3),
            osw(5, 0, 3, 3, 2),
            osw(5, 0, 3, 4, 2),
            osw(5, 0, 3, 3, 1),
            osw(5, 0, 3, 4, 1),
            osw(5, 0, 3, 2, 1),
        ];
        assert_eq!(expected, tile_cover(&params(&transform), 3, ZOOM_RANGE, Some(5)));

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(expected, tile_cover(&params, 3, ZOOM_RANGE, Some(5)));
    }

    // gtest: TileCover.DifferentOverscaledZWrapped
    #[test]
    fn different_overscaled_z_wrapped() {
        let mut transform = transform(512, 512);
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.1, -180.1))).with_zoom(Some(1.0)));

        assert_eq!(
            vec![osw(2, 0, 1, 1, 0), osw(2, 1, 1, 0, 0), osw(2, 0, 1, 1, 1), osw(2, 1, 1, 0, 1)],
            tile_cover(&params(&transform), 1, ZOOM_RANGE, Some(2))
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![osw(2, 0, 1, 1, 0), osw(2, 1, 1, 0, 0), osw(2, 0, 1, 1, 1), osw(2, 1, 1, 0, 1)],
            tile_cover(&params, 1, ZOOM_RANGE, Some(2))
        );
    }

    // gtest: TileCover.FlippedY
    #[test]
    fn flipped_y() {
        let mut transform = transform(512, 512);
        transform.set_viewport_mode(ViewportMode::FlippedY);
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.2, -0.1))).with_zoom(Some(1.0)));

        assert_eq!(
            vec![os(1, 0, 0), os(1, 1, 0), os(1, 0, 1), os(1, 1, 1)],
            tile_cover(&params(&transform), 1, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(vec![os(1, 0, 0), os(1, 1, 0), os(1, 0, 1), os(1, 1, 1)], tile_cover(&params, 1, ZOOM_RANGE, None));
    }

    // gtest: TileCover.FlippedYPitch
    #[test]
    fn flipped_y_pitch() {
        let mut transform = transform(512, 512);
        transform.set_viewport_mode(ViewportMode::FlippedY);
        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(ll(0.1, -0.1)))
                .with_zoom(Some(2.0))
                .with_bearing(Some(5.0))
                .with_pitch(Some(40.0)),
        );

        assert_eq!(
            vec![os(2, 1, 1), os(2, 2, 1), os(2, 1, 2), os(2, 2, 2)],
            tile_cover(&params(&transform), 2, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(vec![os(2, 1, 2), os(2, 2, 2), os(2, 1, 1), os(2, 2, 1)], tile_cover(&params, 2, ZOOM_RANGE, None));
    }

    // gtest: TileCover.FlippedYHelsinki
    #[test]
    fn flipped_y_helsinki() {
        let mut transform = transform(512, 512);
        transform.set_viewport_mode(ViewportMode::FlippedY);
        transform
            .jump_to(&CameraOptions::default().with_center(Some(ll(60.167231, 24.942063))).with_zoom(Some(11.447425)));

        assert_eq!(
            vec![os(11, 1165, 592), os(11, 1166, 592), os(11, 1165, 593), os(11, 1166, 593)],
            tile_cover(&params(&transform), 11, ZOOM_RANGE, None)
        );

        let mut params = params(&transform);
        params.tile_lod_mode = TileLodMode::Distance;
        assert_eq!(
            vec![os(11, 1165, 592), os(11, 1166, 592), os(11, 1165, 593), os(11, 1166, 593)],
            tile_cover(&params, 11, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.SanFranciscoZ0
    #[test]
    fn san_francisco_z0() {
        assert_eq!(vec![uw(0, 0, 0)], tile_cover_bounds(&san_francisco(), 0));
    }

    // gtest: TileCover.SanFranciscoZ10
    #[test]
    fn san_francisco_z10() {
        assert_eq!(
            vec![uw(10, 163, 395), uw(10, 163, 396), uw(10, 164, 395), uw(10, 164, 396)],
            tile_cover_bounds(&san_francisco(), 10)
        );
    }

    // gtest: TileCover.SanFranciscoZ0Wrapped
    #[test]
    fn san_francisco_z0_wrapped() {
        assert_eq!(vec![uw(0, 1, 0)], tile_cover_bounds(&san_francisco_wrapped(), 0));
    }

    // gtest: TileCover.GeomPoint
    #[test]
    fn geom_point() {
        let point = p(-122.5744, 37.6609);

        assert_eq!(vec![uw(2, 0, 1)], tile_cover_geometry(&Geometry::Point(point), 2).unwrap());
    }

    // gtest: TileCover.GeomMultiPoint
    #[test]
    fn geom_multi_point() {
        let points: MultiPoint<f64> = points(&[(-122.5, 37.76), (-122.4, 37.76)]);

        assert_eq!(
            vec![uw(20, 167480, 405351), uw(20, 167772, 405351)],
            tile_cover_geometry(&Geometry::MultiPoint(points), 20).unwrap()
        );
    }

    // gtest: TileCover.GeomLineZ10
    #[test]
    fn geom_line_z10() {
        let line: LineString<f64> =
            points(&[(-121.49368286132812, 38.57903714667459), (-122.4422836303711, 37.773157169570695)]);
        let line_cover = tile_cover_geometry(&Geometry::LineString(line), 10).unwrap();
        assert_eq!(
            vec![
                uw(10, 166, 392),
                uw(10, 165, 393),
                uw(10, 166, 393),
                uw(10, 164, 394),
                uw(10, 165, 394),
                uw(10, 163, 395),
                uw(10, 164, 395),
            ],
            line_cover
        );
    }

    // gtest: TileCover.GeomLineRegression11870
    #[test]
    fn geom_line_regression11870() {
        let line: LineString<f64> = points(&[
            (-121.5063900000001, 40.470099999999945),
            (-121.5065300000001, 40.470369999999946),
            (-121.5065900000001, 40.470519999999944),
        ]);
        let line_cover = tile_cover_geometry(&Geometry::LineString(line), 14).unwrap();
        assert_eq!(vec![uw(14, 2662, 6174)], line_cover);
    }

    // gtest: TileCover.WrappedGeomLineZ10
    #[test]
    fn wrapped_geom_line_z10() {
        let line: LineString<f64> =
            points(&[(-179.93342914581299, 38.892101707724315), (-180.02394485473633, 38.89203490311832)]);
        let line_cover = tile_cover_geometry(&Geometry::LineString(line), 10).unwrap();
        assert_eq!(vec![uw(10, -1, 391), uw(10, 0, 391)], line_cover);

        let line: LineString<f64> =
            points(&[(179.93342914581299, 38.892101707724315), (180.02394485473633, 38.89203490311832)]);
        let line_cover = tile_cover_geometry(&Geometry::LineString(line), 10).unwrap();
        assert_eq!(vec![uw(10, 1023, 391), uw(10, 1024, 391)], line_cover);
    }

    // gtest: TileCover.GeomMultiLineString
    #[test]
    fn geom_multi_line_string() {
        let geom: MultiLineString<f64> =
            vec![points(&[(-122.5, 37.76), (-122.4, 37.76)]), points(&[(-122.5, 37.72), (-122.4, 37.72)])];

        assert_eq!(
            vec![
                uw(14, 2616, 6333),
                uw(14, 2617, 6333),
                uw(14, 2618, 6333),
                uw(14, 2619, 6333),
                uw(14, 2620, 6333),
                uw(14, 2621, 6333),
                uw(14, 2616, 6335),
                uw(14, 2617, 6335),
                uw(14, 2618, 6335),
                uw(14, 2619, 6335),
                uw(14, 2620, 6335),
                uw(14, 2621, 6335),
            ],
            tile_cover_geometry(&Geometry::MultiLineString(geom), 14).unwrap()
        );
    }

    fn europe_ring() -> LinearRing<f64> {
        points(&[
            (5.09765625, 53.067626642387374),
            (2.373046875, 43.389081939117496),
            (-4.74609375, 48.45835188280866),
            (-1.494140625, 37.09023980307208),
            (22.587890625, 36.24427318493909),
            (31.640625, 46.13417004624326),
            (17.841796875, 54.7246201949245),
            (5.09765625, 53.067626642387374),
        ])
    }

    // gtest: TileCover.GeomPolygon
    #[test]
    fn geom_polygon() {
        let polygon: Polygon<f64> = vec![
            europe_ring(),
            points(&[
                (19.6875, 49.66762782262194),
                (22.8515625, 43.51668853502906),
                (13.623046875, 45.089035564831036),
                (16.34765625, 39.095962936305476),
                (5.185546875, 41.244772343082076),
                (8.701171874999998, 50.233151832472245),
                (19.6875, 49.66762782262194),
            ]),
        ];

        let results = tile_cover_geometry(&Geometry::Polygon(polygon), 8).unwrap();

        assert!(results.contains(&uw(8, 134, 87)));
        assert!(results.contains(&uw(8, 139, 87)));
        // Should have a hole
        assert!(!results.contains(&uw(8, 136, 87)));
    }

    // gtest: TileCover.GeomMultiPolygon
    #[test]
    fn geom_multi_polygon() {
        let multi_polygon: MultiPolygon<f64> = vec![
            vec![europe_ring()],
            vec![points(&[
                (59.150390625, 45.460130637921004),
                (65.126953125, 41.11246878918088),
                (69.169921875, 47.45780853075031),
                (63.896484375, 50.064191736659104),
                (59.150390625, 45.460130637921004),
            ])],
        ];
        let results = tile_cover_geometry(&Geometry::MultiPolygon(multi_polygon), 8).unwrap();

        assert_eq!(424, results.len());
        assert!(results.contains(&uw(8, 139, 87)));
        assert!(results.contains(&uw(8, 136, 87)));
        assert!(results.contains(&uw(8, 174, 94)));
    }

    // gtest: TileCover.GeomSanFranciscoPoly
    #[test]
    fn geom_san_francisco_poly() {
        let san_francisco_geom: Polygon<f64> = vec![points(&[
            (-122.5143814086914, 37.779127216982424),
            (-122.50811576843262, 37.72721239056709),
            (-122.50313758850099, 37.70820178063929),
            (-122.3938751220703, 37.707454835665274),
            (-122.37567901611328, 37.70663997801684),
            (-122.36297607421874, 37.71343018466285),
            (-122.354736328125, 37.727280276860036),
            (-122.36469268798828, 37.73868429065797),
            (-122.38014221191408, 37.75442980295571),
            (-122.38391876220702, 37.78753873820529),
            (-122.35919952392578, 37.8065289741725),
            (-122.35679626464844, 37.820632846207864),
            (-122.3712158203125, 37.835276322922695),
            (-122.3818588256836, 37.82958198283902),
            (-122.37190246582031, 37.80788523279169),
            (-122.38735198974608, 37.791337175930686),
            (-122.40966796874999, 37.812767557570204),
            (-122.46425628662108, 37.807071480609274),
            (-122.46803283691405, 37.810326435534755),
            (-122.47901916503906, 37.81168262440736),
            (-122.48966217041016, 37.78916666399649),
            (-122.50579833984375, 37.78781006166096),
            (-122.5143814086914, 37.779127216982424),
        ])];

        assert_eq!(
            vec![
                uw(12, 654, 1582),
                uw(12, 655, 1582),
                uw(12, 654, 1583),
                uw(12, 655, 1583),
                uw(12, 654, 1584),
                uw(12, 655, 1584),
            ],
            tile_cover_geometry(&Geometry::Polygon(san_francisco_geom), 12).unwrap()
        );
    }

    // gtest: TileCover.GeomInvalid
    #[test]
    fn geom_invalid() {
        // EXPECT_THROW(..., std::domain_error): the LatLng constructor's error.
        let point = p(-122.5744, 97.6609);
        assert_eq!(
            Err(mln_util::Error::invalid_argument("latitude must be between -90 and 90")),
            tile_cover_geometry(&Geometry::Point(point), 2)
        );

        let bad_line: LineString<f64> = vec![p(1.0, 35.0)];
        assert_eq!(Vec::<UnwrappedTileID>::new(), tile_cover_geometry(&Geometry::LineString(bad_line), 16).unwrap());

        let bad_poly: Polygon<f64> = vec![vec![p(1.0, 35.0)]];
        assert_eq!(Vec::<UnwrappedTileID>::new(), tile_cover_geometry(&Geometry::Polygon(bad_poly), 16).unwrap());

        // Should handle open polygons.
        let bad_poly: Polygon<f64> = vec![points(&[(1.0, 34.2), (1.0, 34.4), (0.5, 34.3)])];
        assert_eq!(
            vec![uw(10, 513, 407), uw(10, 514, 407), uw(10, 513, 408), uw(10, 514, 408)],
            tile_cover_geometry(&Geometry::Polygon(bad_poly), 10).unwrap()
        );
    }

    // gtest: TileCover.CoveringZoomLevelRoundsForRasterSources
    #[test]
    fn covering_zoom_level_rounds_for_raster_sources() {
        assert_eq!(4, covering_zoom_level(4.6, SourceType::Vector, constants::TILE_SIZE_I));
        assert_eq!(4, covering_zoom_level(4.6, SourceType::GeoJSON, constants::TILE_SIZE_I));
        assert_eq!(5, covering_zoom_level(4.6, SourceType::Raster, constants::TILE_SIZE_I));
        assert_eq!(5, covering_zoom_level(4.6, SourceType::RasterDEM, constants::TILE_SIZE_I));
        assert_eq!(5, covering_zoom_level(4.6, SourceType::Video, constants::TILE_SIZE_I));
        assert_eq!(4, covering_zoom_level(4.4, SourceType::RasterDEM, constants::TILE_SIZE_I));
        assert_eq!(6, covering_zoom_level(4.6, SourceType::RasterDEM, 256));
    }

    // DISABLED_FuzzPoly and DISABLED_FuzzLine loop forever in C++ (manual
    // fuzz harnesses, re-seeding `rand` with the time on every round). The
    // ports are ignored like the C++ tests are disabled, use the fixed-seed
    // `Lcg` instead of `rand`, and stop after a fixed number of rounds so
    // `--include-ignored` terminates.
    const FUZZ_ROUNDS: usize = 20;

    // gtest: TileCover.DISABLED_FuzzPoly
    #[test]
    #[ignore]
    fn disabled_fuzz_poly() {
        let mut rng = Lcg(1);
        for _ in 0..FUZZ_ROUNDS {
            let len = rng.rand() % 10000 + 3;
            let mut polygon: Polygon<f64> = Vec::new();

            let mut num_rings = 1;
            num_rings += rng.rand() % 5;
            while num_rings > 0 {
                let mut ring: LinearRing<f64> = Vec::new();
                for _ in 0..len {
                    let x = (rng.rand() % 180) as f64;
                    let y = (rng.rand() % 90) as f64;

                    ring.push(p(x, y));
                }
                polygon.push(ring);
                num_rings -= 1;
            }

            eprint!(".");
            let mut tc = TileCover::new_from_geometry(&Geometry::Polygon(polygon), 5, true).unwrap();
            while tc.next().is_some() {}
        }
    }

    // gtest: TileCover.DISABLED_FuzzLine
    #[test]
    #[ignore]
    fn disabled_fuzz_line() {
        let mut rng = Lcg(1);
        for _ in 0..FUZZ_ROUNDS {
            let len = rng.rand() % 10000 + 3;
            let mut mls: MultiLineString<f64> = Vec::new();

            let mut num_lines = 1;
            num_lines += rng.rand() % 5;
            while num_lines > 0 {
                let mut line: LineString<f64> = Vec::new();
                for _ in 0..len {
                    let x = (rng.rand() % 180) as f64;
                    let y = (rng.rand() % 90) as f64;

                    line.push(p(x, y));
                }
                mls.push(line);
                num_lines -= 1;
            }

            eprint!(".");
            let mut tc = TileCover::new_from_geometry(&Geometry::MultiLineString(mls), 5, true).unwrap();
            while tc.next().is_some() {}
        }
    }

    /// `setUpGlobe(transform, size, center, zoom, pitch, bearing)` (C++
    /// defaults `pitch` and `bearing` to 0).
    fn set_up_globe(transform: &mut Transform, size: Size, center: LatLng, zoom: f64, pitch: f64, bearing: f64) {
        transform.resize(size).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.set_max_pitch(80.0);
        transform.jump_to(
            &CameraOptions::default()
                .with_center(Some(center))
                .with_zoom(Some(zoom))
                .with_pitch(Some(pitch))
                .with_bearing(Some(bearing)),
        );
        assert!(transform.get_state().is_globe_rendering());
    }

    // The globe cases of GL JS `covering_tiles.test.ts`, same cameras, same tiles in the same order.
    // gtest: TileCover.GlobeZoomedOut
    #[test]
    fn globe_zoomed_out() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(0.0, 0.0), -1.0, 0.0, 0.0);
        assert_eq!(vec![os(0, 0, 0)], tile_cover(&params(&transform), 0, ZOOM_RANGE, None));
    }

    // gtest: TileCover.GlobeZoomedIn
    #[test]
    fn globe_zoomed_in() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(0.01, -0.02), 3.0, 0.0, 0.0);
        assert_eq!(
            vec![os(3, 3, 3), os(3, 3, 4), os(3, 4, 3), os(3, 4, 4)],
            tile_cover(&params(&transform), 3, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.GlobeZoomedInLargeViewport
    #[test]
    fn globe_zoomed_in_large_viewport() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(512, 512), ll(0.01, -0.02), 3.0, 0.0, 0.0);
        assert_eq!(
            vec![
                os(3, 3, 3),
                os(3, 3, 4),
                os(3, 4, 3),
                os(3, 4, 4),
                os(3, 2, 3),
                os(3, 2, 4),
                os(3, 5, 3),
                os(3, 5, 4),
            ],
            tile_cover(&params(&transform), 3, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.GlobePitched
    #[test]
    fn globe_pitched() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(0.001, -0.002), 8.0, 80.0, 0.0);
        assert_eq!(
            vec![os(6, 32, 31), os(6, 31, 31), os(10, 511, 512), os(10, 512, 512), os(10, 511, 513), os(10, 512, 513)],
            tile_cover(&params(&transform), 8, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.GlobePitchedAndRotated
    #[test]
    fn globe_pitched_and_rotated() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(0.001, -0.002), 8.0, 80.0, 45.0);
        assert_eq!(
            vec![os(7, 64, 64), os(7, 64, 63), os(7, 63, 63), os(10, 510, 512), os(10, 511, 512), os(10, 511, 513)],
            tile_cover(&params(&transform), 8, ZOOM_RANGE, None)
        );
    }

    // GL JS `coveringTiles` on the globe for sources whose zoom range or tile size moves the zoom they load at: the
    // level of detail falls off from the map's zoom and only then meets the source's maximum, and raster sources round.
    // GL JS builds its globe frustum from a float32 copy of the inverse matrix, which at these zooms drops the tile
    // at the far top edge of the view; the lists are GL JS's with the frustum built in double precision, as here.
    // gtest: TileCover.GlobeSourceZoomsLikeGLJS
    #[test]
    fn globe_source_zooms_like_gljs() {
        struct Case {
            name: &'static str,
            zoom: f64,
            pitch: f64,
            tile_size: u16,
            max_zoom: u8,
            type_: SourceType,
            expected: Vec<CanonicalTileID>,
        }
        let cases = vec![
            Case {
                name: "overzoomed",
                zoom: 10.5,
                pitch: 60.0,
                tile_size: 512,
                max_zoom: 8,
                type_: SourceType::Vector,
                expected: vec![cn(8, 135, 113), cn(8, 134, 113), cn(8, 134, 112)],
            },
            Case {
                name: "overzoomed to the horizon",
                zoom: 10.5,
                pitch: 75.0,
                tile_size: 512,
                max_zoom: 8,
                type_: SourceType::Vector,
                expected: vec![cn(8, 135, 113), cn(8, 134, 113), cn(8, 135, 112), cn(8, 134, 112), cn(5, 16, 13)],
            },
            Case {
                name: "low max zoom",
                zoom: 10.0,
                pitch: 60.0,
                tile_size: 512,
                max_zoom: 4,
                type_: SourceType::Vector,
                expected: vec![cn(4, 8, 7), cn(4, 8, 6)],
            },
            Case {
                name: "raster rounding up",
                zoom: 5.6,
                pitch: 60.0,
                tile_size: 256,
                max_zoom: 22,
                type_: SourceType::Raster,
                expected: vec![
                    cn(7, 67, 56),
                    cn(7, 67, 57),
                    cn(7, 68, 56),
                    cn(7, 66, 56),
                    cn(7, 68, 57),
                    cn(7, 66, 57),
                    cn(7, 67, 58),
                    cn(7, 69, 56),
                    cn(7, 68, 58),
                    cn(7, 66, 58),
                    cn(7, 65, 56),
                    cn(6, 35, 26),
                    cn(6, 34, 27),
                    cn(6, 34, 26),
                    cn(6, 33, 27),
                    cn(6, 33, 26),
                    cn(6, 32, 27),
                    cn(6, 32, 26),
                    cn(5, 18, 12),
                    cn(5, 17, 12),
                    cn(5, 16, 12),
                    cn(5, 15, 12),
                ],
            },
            Case {
                name: "raster rounding down",
                zoom: 5.3,
                pitch: 60.0,
                tile_size: 256,
                max_zoom: 22,
                type_: SourceType::Raster,
                expected: vec![
                    cn(6, 33, 28),
                    cn(6, 34, 28),
                    cn(6, 33, 27),
                    cn(6, 34, 27),
                    cn(6, 32, 28),
                    cn(6, 32, 27),
                    cn(6, 33, 26),
                    cn(6, 35, 27),
                    cn(6, 34, 26),
                    cn(6, 32, 26),
                    cn(6, 35, 26),
                    cn(5, 18, 12),
                    cn(5, 17, 12),
                    cn(5, 16, 12),
                    cn(5, 15, 12),
                    cn(7, 66, 58),
                    cn(7, 67, 58),
                    cn(7, 68, 58),
                ],
            },
        ];
        for c in &cases {
            // SCOPED_TRACE(c.name): every assertion below names the case.
            let mut transform = new_transform();
            set_up_globe(&mut transform, Size::new(512, 512), ll(20.0, 10.0), c.zoom, c.pitch, 0.0);
            // As TilePyramid asks for them.
            let covering_zoom: i32 = covering_zoom_level(c.zoom, c.type_, c.tile_size);
            let ideal_zoom = std::cmp::min(i32::from(c.max_zoom), covering_zoom) as u8;
            let raster = c.type_ == SourceType::Raster;
            let mut parameters = TileCoverParameters::new(transform.get_state().clone());
            parameters.requested_zoom = Some(c.zoom + mln_math::log2(512.0 / f64::from(c.tile_size)));
            parameters.round_zoom = raster;
            let tiles = tile_cover(
                &parameters,
                ideal_zoom,
                Range::new(0, c.max_zoom),
                Some(if raster { ideal_zoom } else { covering_zoom as u8 }),
            );
            let mut canonical = Vec::new();
            for tile in &tiles {
                canonical.push(tile.canonical);
                assert_eq!(0, tile.wrap, "{}", c.name);
            }
            assert_eq!(c.expected, canonical, "{}", c.name);
        }
    }

    // GL JS `globe_covering_tiles.test.ts` "covers the tiles around the south pole", the tiles in its order: past the
    // Mercator range there are no tiles, so they are chosen around the center brought back to the edge of the range.
    // gtest: TileCover.GlobeAroundThePole
    #[test]
    fn globe_around_the_pole() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(512, 512), ll(-90.0, 0.0), 1.6, 0.0, 0.0);
        assert_eq!(-90.0, transform.get_lat_lng(WrapMode::Wrapped).latitude());
        assert_eq!(
            vec![
                os(5, 16, 31),
                os(5, 15, 31),
                os(5, 17, 31),
                os(5, 14, 31),
                os(5, 18, 31),
                os(5, 13, 31),
                os(5, 18, 30),
                os(5, 13, 30),
                os(5, 19, 31),
                os(5, 12, 31),
                os(5, 19, 30),
                os(5, 12, 30),
                os(5, 19, 29),
                os(5, 12, 29),
                os(5, 20, 31),
                os(5, 11, 31),
                os(5, 20, 30),
                os(5, 11, 30),
                os(5, 20, 29),
                os(5, 11, 29),
                os(5, 21, 31),
                os(5, 10, 31),
                os(5, 21, 30),
                os(5, 10, 30),
                os(5, 22, 31),
                os(5, 9, 31),
                os(5, 23, 31),
                os(5, 8, 31),
                os(5, 24, 31),
                os(5, 7, 31),
                os(5, 25, 31),
                os(5, 6, 31),
                os(5, 26, 31),
                os(5, 5, 31),
                os(5, 26, 30),
                os(5, 5, 30),
                os(5, 27, 31),
                os(5, 4, 31),
                os(5, 27, 30),
                os(5, 4, 30),
                os(5, 27, 29),
                os(5, 4, 29),
                os(5, 28, 31),
                os(5, 3, 31),
                os(5, 28, 30),
                os(5, 3, 30),
                os(5, 28, 29),
                os(5, 3, 29),
                os(5, 29, 31),
                os(5, 2, 31),
                os(5, 29, 30),
                os(5, 2, 30),
                os(5, 30, 31),
                os(5, 1, 31),
                os(5, 31, 31),
                os(5, 0, 31),
            ],
            tile_cover(&params(&transform), 5, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.GlobeAntimeridian
    #[test]
    fn globe_antimeridian() {
        let mut east = new_transform();
        set_up_globe(&mut east, Size::new(128, 128), ll(-0.001, 179.99), 5.0, 0.0, 0.0);
        assert_eq!(
            vec![os(5, 31, 16), os(5, 31, 15), osw(5, 1, 5, 0, 16), osw(5, 1, 5, 0, 15)],
            tile_cover(&params(&east), 5, ZOOM_RANGE, None)
        );

        let mut west = new_transform();
        set_up_globe(&mut west, Size::new(128, 128), ll(0.001, -179.99), 5.0, 0.0, 0.0);
        assert_eq!(
            vec![os(5, 0, 15), os(5, 0, 16), osw(5, -1, 5, 31, 15), osw(5, -1, 5, 31, 16)],
            tile_cover(&params(&west), 5, ZOOM_RANGE, None)
        );
    }

    // gtest: TileCover.GlobeBelowZoomZero
    #[test]
    fn globe_below_zoom_zero() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(80.0, 0.0), -0.5, 0.0, 0.0);
        assert_eq!(0, transform.get_state().get_integer_zoom());
        assert_eq!(vec![os(0, 0, 0)], tile_cover(&params(&transform), 0, Range::new(0, 0), None));
    }

    // gtest: TileCover.GlobeAtTheHandOffZoom
    #[test]
    fn globe_at_the_hand_off_zoom() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(128, 128), ll(-0.087, -179.73), 11.0, 0.0, 0.0);
        assert_eq!(vec![os(11, 1, 1024)], tile_cover(&params(&transform), 11, Range::new(0, 15), None));
    }

    // gtest: TileCover.GlobeHidesTheFarSide
    #[test]
    fn globe_hides_the_far_side() {
        let mut transform = new_transform();
        set_up_globe(&mut transform, Size::new(512, 512), ll(0.0, 0.0), 3.0, 0.0, 0.0);
        let cover = tile_cover(&params(&transform), 3, ZOOM_RANGE, None);
        let has = |x: u32, y: u32| cover.contains(&os(3, x, y));
        assert!(has(3, 3));
        assert!(has(4, 4));
        // Tiles on the far side of the planet (lng 135..180 at the equator) are not covered.
        assert!(!has(7, 3));
        assert!(!has(0, 4));
        assert!(cover.len() < 64);
    }

    // gtest: TileCover.GlobeZoomZeroIsTheWholePlanet
    #[test]
    fn globe_zoom_zero_is_the_whole_planet() {
        let mut transform = transform(512, 512);
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(40.0, 10.0))).with_zoom(Some(0.0)));
        assert_eq!(vec![os(0, 0, 0)], tile_cover(&params(&transform), 0, ZOOM_RANGE, None));
    }

    // gtest: TileCover.GlobeCoverSurvivesTheAntimeridian
    #[test]
    fn globe_cover_survives_the_antimeridian() {
        let mut transform = transform(512, 512);
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.0, 179.0))).with_zoom(Some(1.0)));
        let mut before = tile_cover(&params(&transform), 1, Range::new(0, 14), None);
        transform.jump_to(&CameraOptions::default().with_center(Some(ll(0.0, -179.0))).with_zoom(Some(1.0)));
        let mut after = tile_cover(&params(&transform), 1, Range::new(0, 14), None);
        before.sort();
        after.sort();
        assert_eq!(before, after);
        assert_eq!(4, after.len());
    }

    // A center spun into another world copy covers the same tiles, in the same order, moved into that copy.
    // gtest: TileCover.GlobeCoverFollowsTheCenterIntoOtherWorldCopies
    #[test]
    fn globe_cover_follows_the_center_into_other_world_copies() {
        let mut transform = transform(512, 512);
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(
            &CameraOptions::default().with_center(Some(ll(10.0, 20.0))).with_zoom(Some(6.0)).with_pitch(Some(50.0)),
        );
        let home = tile_cover(&params(&transform), 6, Range::new(0, 14), None);
        for step in 1..=6 {
            transform.jump_to(&CameraOptions::default().with_center(Some(ll(10.0, 20.0 + 120.0 * f64::from(step)))));
        }
        assert_near(740.0, transform.get_state().get_lat_lng(WrapMode::Unwrapped).longitude(), 1e-9);
        let spun = tile_cover(&params(&transform), 6, Range::new(0, 14), None);
        assert_eq!(home.len(), spun.len());
        for i in 0..home.len() {
            assert_eq!(home[i].canonical, spun[i].canonical, "{i}");
            assert_eq!(home[i].wrap + 2, spun[i].wrap, "{i}");
            assert_eq!(home[i].overscaled_z, spun[i].overscaled_z, "{i}");
        }
    }
}

mod tile_cover_stream {
    use super::*;

    // gtest: TileCoverStream.Arctic
    #[test]
    fn arctic() {
        let bounds = LatLngBounds::hull(ll(84.0, -180.0), ll(70.0, 180.0));
        let zoom = 3;
        let mut tc = TileCover::new_from_bounds(&bounds, zoom);
        let results = tile_cover_bounds(&bounds, zoom);
        let mut t = Vec::new();
        while tc.has_next() {
            t.push(tc.next().unwrap());
        }
        assert_eq!(t.len(), results.len());
    }

    // gtest: TileCoverStream.WorldZ1
    #[test]
    fn world_z1() {
        let zoom = 1;
        let mut tc = TileCover::new_from_bounds(&LatLngBounds::world(), zoom);
        let mut t = Vec::new();
        while tc.has_next() {
            t.push(tc.next().unwrap());
        }
        assert_eq!(vec![uw(1, 0, 0), uw(1, 1, 0), uw(1, 0, 1), uw(1, 1, 1)], t);
    }
}

mod tile_count {
    use super::*;

    // gtest: TileCount.World
    #[test]
    fn world() {
        assert_eq!(1, tile_count_bounds(&LatLngBounds::world(), 0));
        assert_eq!(4, tile_count_bounds(&LatLngBounds::world(), 1));
    }

    // gtest: TileCount.SanFranciscoZ10
    #[test]
    fn san_francisco_z10() {
        assert_eq!(4, tile_count_bounds(&san_francisco(), 10));
    }

    // gtest: TileCount.SanFranciscoWrappedZ10
    #[test]
    fn san_francisco_wrapped_z10() {
        assert_eq!(4, tile_count_bounds(&san_francisco_wrapped(), 10));
    }

    // gtest: TileCount.SanFranciscoZ22
    #[test]
    fn san_francisco_z22() {
        assert_eq!(7254450, tile_count_bounds(&san_francisco(), 22));
    }

    // gtest: TileCount.BoundsCrossingAntimeridian
    #[test]
    fn bounds_crossing_antimeridian() {
        let crossing_bounds = LatLngBounds::hull(ll(-20.9615, -214.309), ll(19.477, -155.830));

        assert_eq!(1, tile_count_bounds(&crossing_bounds, 0));
        assert_eq!(4, tile_count_bounds(&crossing_bounds, 3));
        assert_eq!(8, tile_count_bounds(&crossing_bounds, 4));
    }
}
