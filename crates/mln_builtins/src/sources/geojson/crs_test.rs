//! GeoJSON sources on maps drawn in a registered projection: the CRS cases
//! of MapLibre Native's test/style/source.test.cpp, and those of GL JS's
//! "loadData in a registered projection" (`geojson_worker_source.test.ts`).

use std::sync::Arc;

use geo_types::{Geometry, Point};
use geojson::GeoJson;
use mln_engine::source::{Load, SourceType};
use mln_style::Style;
use mln_style::crs::{Crs, CrsConverter, CrsMapping, TileMatrixSet};
use mln_style::feature::{Feature, Value};
use mln_style::source::SourceData;
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceOptions, GeoJsonSourceType};
use mln_style::tile_id::CanonicalTileID;

use super::{
    GeoJSONData, GeoJsonSource, create_geojson_data, create_geojson_data_in, geojson_data, set_data, set_geojson,
};
use crate::registry;

const POINT_AT_45: &str = r#"{"type": "Point", "coordinates": [45, 45]}"#;

/// Where tile 0/0/0 has the point at 45 E, 45 N in the identity projection,
/// whose square spans -90 to 90 on both axes.
const IDENTITY_45: Point<i16> = Point(geo_types::Coord { x: 6144, y: 2048 });

/// Where tile 0/0/0 has the point at 45 E, 45 N in Mercator.
const MERCATOR_45: Point<i16> = Point(geo_types::Coord { x: 5120, y: 2947 });

fn identity() -> Arc<CrsMapping> {
    Arc::clone(registry().style().projection("identity").expect("identity"))
}

fn geojson(json: &str) -> GeoJson {
    json.parse().expect("GeoJSON")
}

/// The data of `json`, built with `options` in `crs`.
fn build(json: &str, options: GeoJSONOptions, crs: Option<&Arc<CrsMapping>>) -> Arc<dyn GeoJSONData> {
    create_geojson_data_in(geojson(json), &Arc::new(options), crs).expect("data")
}

/// The points of tile 0/0/0 of `data`.
fn root_points(data: &Arc<dyn GeoJSONData>) -> Vec<Point<i16>> {
    data.get_tile(&CanonicalTileID::new(0, 0, 0))
        .iter()
        .map(|feature| match feature.geometry {
            Some(Geometry::Point(point)) => point,
            ref other => panic!("not a point: {other:?}"),
        })
        .collect()
}

// gtest: Source.GeoJSONDataTilesTheDataWhereTheIdentityProjectionPutsIt
#[test]
fn geojson_data_tiles_the_data_where_the_identity_projection_puts_it() {
    let data = build(POINT_AT_45, GeoJSONOptions::default(), Some(&identity()));
    assert_eq!(vec![IDENTITY_45], root_points(&data));
    assert!(data.crs().is_some_and(|crs| Arc::ptr_eq(crs, &identity())));
    let data = create_geojson_data(geojson(POINT_AT_45), &GeoJSONOptions::default_options()).unwrap();
    assert_eq!(vec![MERCATOR_45], root_points(&data));
    assert!(data.crs().is_none());
}

/// A projection that scales lng/lat by ten, over a square 2000 units wide.
struct TimesTen;

impl CrsConverter for TimesTen {
    fn forward(&self, lng_lats: &[[f64; 2]], positions: &mut [[f64; 2]]) {
        for (position, [lng, lat]) in positions.iter_mut().zip(lng_lats) {
            *position = [lng * 10.0, lat * 10.0];
        }
    }

    fn inverse(&self, positions: &[[f64; 2]], lng_lats: &mut [[f64; 2]]) {
        for (lng_lat, [x, y]) in lng_lats.iter_mut().zip(positions) {
            *lng_lat = [x / 10.0, y / 10.0];
        }
    }
}

// GL JS: "tiles the data where the projection handler puts it".
#[test]
fn geojson_data_tiles_the_data_where_a_registered_projection_puts_it() {
    let crs = Crs {
        name: "test:times-ten".to_owned(),
        converter: Box::new(TimesTen),
        tile_matrix_set: TileMatrixSet { origin: [-1000.0, 1000.0], extent_at_zoom0: 2000.0 },
    };
    let data = build(POINT_AT_45, GeoJSONOptions::default(), Some(&Arc::new(CrsMapping::new(Arc::new(crs)))));
    // 450, 450 is 0.725 across the square and 0.275 down it.
    assert_eq!(vec![Point::new(5939, 2253)], root_points(&data));
}

// gtest: Source.GeoJSONDataLeavesOutPointsOutsideTheWorldSquareOfARegisteredProjection
// GL JS: "leaves out points outside the grid square instead of wrapping them
// around or clamping them onto its top edge".
#[test]
fn geojson_data_leaves_out_points_outside_the_world_square_of_a_registered_projection() {
    let data = build(
        r#"{"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [-30, -30]}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [150, 30]}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [45, 135]}}]}"#,
        GeoJSONOptions::default(),
        Some(&identity()),
    );
    assert_eq!(vec![Point::new(2731, 5461)], root_points(&data));
}

/// Two points at 45 E, 45 N, clustered in the identity projection, and the
/// ID of their cluster.
fn cluster_at_45() -> (Arc<dyn GeoJSONData>, u32) {
    let data = build(
        r#"{"type": "FeatureCollection", "features": [
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [45, 45]}},
            {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [45, 45]}}]}"#,
        GeoJSONOptions { cluster: true, ..GeoJSONOptions::default() },
        Some(&identity()),
    );
    let tile = data.get_tile(&CanonicalTileID::new(0, 0, 0));
    let [cluster] = &tile[..] else { panic!("one cluster: {tile:?}") };
    assert_eq!(Some(Geometry::Point(IDENTITY_45)), cluster.geometry);
    let Some(Value::Uint(id)) = cluster.properties.get("cluster_id") else { panic!("a cluster ID") };
    (data, *id as u32)
}

/// Asserts that every feature of `features` is a point at 45 E, 45 N.
#[track_caller]
fn assert_at_45(features: &[Feature<f64>]) {
    for feature in features {
        let Some(Geometry::Point(point)) = feature.geometry else { panic!("not a point: {feature:?}") };
        assert!((point.x() - 45.0).abs() < 1e-9 && (point.y() - 45.0).abs() < 1e-9, "{point:?}");
    }
}

// gtest: Source.GeoJSONDataReturnsClusterLeavesInLatLngInARegisteredProjection
// GL JS: "returns cluster leaves in lng/lat".
#[test]
fn geojson_data_returns_cluster_leaves_in_lng_lat_in_a_registered_projection() {
    let (data, id) = cluster_at_45();
    let leaves = data.get_leaves(id, 10, 0).expect("leaves");
    assert_eq!(2, leaves.len());
    assert_at_45(&leaves);
}

#[test]
fn geojson_data_returns_cluster_children_in_lng_lat_in_a_registered_projection() {
    let (data, id) = cluster_at_45();
    let children = data.get_children(id).expect("children");
    assert_eq!(2, children.len());
    assert_at_45(&children);
}

// Clustered data that isn't all points fails as it does in lng/lat.
#[test]
fn clustered_data_with_a_feature_that_is_not_a_point_fails_as_in_lng_lat() {
    let line = r#"{"type": "FeatureCollection", "features": [
        {"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [0, 0]}},
        {"type": "Feature", "properties": {}, "geometry": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}}]}"#;
    let options = Arc::new(GeoJSONOptions { cluster: true, ..GeoJSONOptions::default() });
    let error = |crs| create_geojson_data_in(geojson(line), &options, crs).unwrap_err();
    assert_eq!(error(None).message, error(Some(&identity())).message);
}

/// The style of `json`, against the built-ins' registry.
fn style(json: &str) -> Style {
    Style::from_json(registry().style(), json).expect("a style")
}

/// The data of the style's GeoJSON source `id`.
fn source_data<'a>(style: &'a Style, id: &str) -> &'a SourceData<GeoJsonSourceType> {
    SourceData::<GeoJsonSourceType>::recover(style.get_source(id).expect("the source")).expect("a GeoJSON source")
}

/// What describing the style's GeoJSON source `id`'s options and `fetched`
/// gives.
fn describe(style: &Style, id: &str, fetched: Option<&str>) -> Arc<dyn GeoJSONData> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let fetched = fetched.map(|json| Arc::new(json.as_bytes().to_vec()));
    GeoJsonSource::describe(
        &source_data(style, id).options,
        fetched,
        Box::new(move |described| {
            let _ = sender.send(described);
        }),
    );
    receiver.recv().expect("an answer").expect("described").expect("data")
}

// gtest: Source.GeoJSONSourceTilesInlineDataInTheStylesProjection
// gtest: Source.GeoJSONSourceTilesDataFromAURLInTheStylesProjection
// gtest: Source.GeoJSONSourceTilesDataInLatLngInABuiltInProjection
#[test]
fn a_sources_geojson_is_tiled_in_its_styles_projection() {
    for (projection, at_45) in [(r#""identity""#, IDENTITY_45), (r#""mercator""#, MERCATOR_45)] {
        let style = style(&format!(
            r#"{{"version": 8, "projection": {{"type": {projection}}},
                "sources": {{
                    "inline": {{"type": "geojson", "data": {POINT_AT_45}}},
                    "url": {{"type": "geojson", "data": "http://example.com/point.geojson"}}}},
                "layers": []}}"#
        ));
        assert_eq!(vec![at_45], root_points(&describe(&style, "inline", None)), "{projection}");
        assert_eq!(vec![at_45], root_points(&describe(&style, "url", Some(POINT_AT_45))), "{projection}");
    }
}

/// `options` keeping the point at 45 E, 45 N, for data tiled as `data`.
fn keeping(options: &GeoJsonSourceOptions, data: Arc<dyn GeoJSONData>) -> SourceData<GeoJsonSourceType> {
    let options = GeoJsonSourceOptions { kept: Some(Arc::new(geojson(POINT_AT_45))), ..options.clone() };
    SourceData::with_description(options, Arc::new(data))
}

// MapLibre Native's "GeoJSON is tiled again when the projection changes":
// a source that keeps the GeoJSON set on it describes it again while its
// data is tiled for another projection than its style's, in the style's.
#[test]
fn kept_geojson_is_described_again_in_the_styles_projection() {
    let style = style(
        r#"{"version": 8, "projection": {"type": "identity"},
            "sources": {"set": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
            "layers": []}"#,
    );
    let options = GeoJsonSourceOptions { inline: None, ..source_data(&style, "set").options.clone() };
    let mercator = create_geojson_data(geojson(POINT_AT_45), &options.options).unwrap();
    assert!(matches!(GeoJsonSource::load(&keeping(&options, mercator)), Load::Describe));
    let in_identity = build(POINT_AT_45, GeoJSONOptions::default(), Some(&identity()));
    assert!(matches!(GeoJsonSource::load(&keeping(&options, Arc::clone(&in_identity))), Load::Done));

    let (sender, receiver) = std::sync::mpsc::channel();
    let options = GeoJsonSourceOptions { kept: Some(Arc::new(geojson(POINT_AT_45))), ..options };
    GeoJsonSource::describe(
        &options,
        None,
        Box::new(move |described| {
            let _ = sender.send(described);
        }),
    );
    let described = receiver.recv().unwrap().unwrap().expect("data");
    assert_eq!(vec![IDENTITY_45], root_points(&described));
}

// The runtime's data keeps its GeoJSON while the style is in a registered
// projection, or while the data is tiled for another projection than the
// style's, and not otherwise.
#[test]
fn set_geojson_keeps_the_geojson_to_tile_again() {
    for (projection, crs, keeps) in [
        (r#""identity""#, Some(identity()), true),
        (r#""mercator""#, None, false),
        (r#""mercator""#, Some(identity()), true),
    ] {
        let mut style = style(&format!(
            r#"{{"version": 8, "projection": {{"type": {projection}}},
                "sources": {{"set": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": []}}}}}},
                "layers": []}}"#
        ));
        let data = build(POINT_AT_45, GeoJSONOptions::default(), crs.as_ref());
        style.apply(set_geojson("set", Arc::new(geojson(POINT_AT_45)), Arc::clone(&data))).unwrap();
        let source = source_data(&style, "set");
        assert_eq!(keeps, source.options.kept.is_some(), "{projection}, {:?}", crs.map(|crs| crs.crs().name.clone()));
        assert!(source.options.inline.is_none());
        assert!(Arc::ptr_eq(&data, geojson_data(style.get_source("set").unwrap()).unwrap()));
    }
}

// Data that the app built itself replaces what a source kept.
#[test]
fn set_data_drops_the_kept_geojson() {
    let mut style = style(
        r#"{"version": 8, "projection": {"type": "identity"},
            "sources": {"set": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
            "layers": []}"#,
    );
    let data = build(POINT_AT_45, GeoJSONOptions::default(), Some(&identity()));
    style.apply(set_geojson("set", Arc::new(geojson(POINT_AT_45)), Arc::clone(&data))).unwrap();
    style.apply(set_data("set", Some(data))).unwrap();
    assert!(source_data(&style, "set").options.kept.is_none());
}
