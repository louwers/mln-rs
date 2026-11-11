// Ports test/api/query.test.cpp (a module of the gtest_api test crate).

use std::sync::Arc;

use feature::{PropertyMap, Value};
use geojson::{GeoJSON, Geometry, Point};
use mln_actor::RunLoop;
use mln_map::{CameraOptions, MapMode};
use mln_renderer::query::{FeatureExtensionValue, RenderedQueryOptions, SourceQueryOptions};
use mln_storage::Kind;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::PropertyValue;
use mln_style::conversion::{Convertible, Error, convert};
use mln_style::expression::Value as ExpressionValue;
use mln_style::expression::dsl::{eq, get_str, gt, id, literal, literal_str, ne, number};
use mln_style::expression::expression::FeatureState;
use mln_style::filter::Filter;
use mln_style::layers::circle_layer::CircleLayer;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style::source::Source;
use mln_style::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use mln_util::exception::RuntimeError;
use mln_util::feature::Feature;
use mln_util::geo::{LatLng, ScreenCoordinate};

use crate::style_image;
use crate::support::*;

/// The `QueryTest` fixture: a static 256x256 map with
/// test/fixtures/api/query_style.json and the "test-icon" image, rendered
/// once.
struct QueryTest(MapTest);

impl QueryTest {
    fn new() -> Self {
        let run_loop = RunLoop::new();
        // TODO(port): `StubFileSource()` uses the MapTiler tile server options.
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        let test = QueryTest(MapTest::create(run_loop, file_source, 1.0, MapMode::Static));
        test.map().get_style_mut().load_json(&read_file("test/fixtures/api/query_style.json"));
        test.map().get_style_mut().add_image(style_image(
            "test-icon",
            decode_image("test/fixtures/sprites/default_marker.png"),
            1.0,
        ));

        test.0.render();
        test
    }

    fn map(&self) -> std::cell::RefMut<'_, mln_map::Map> {
        self.0.map()
    }

    fn render(&self) -> mln_headless::RenderResult {
        self.0.render()
    }

    /// `frontend.getRenderer()->queryRenderedFeatures(point, options)`.
    fn query_rendered_features(&self, point: &ScreenCoordinate, options: &RenderedQueryOptions) -> Vec<Feature> {
        self.0.frontend.with_renderer(|renderer| renderer.query_rendered_features_at_point(point, options)).unwrap()
    }

    /// `frontend.getRenderer()->querySourceFeatures(sourceID, options)`.
    fn query_source_features(&self, source_id: &str, options: &SourceQueryOptions) -> Vec<Feature> {
        self.0.frontend.with_renderer(|renderer| renderer.query_source_features(source_id, options)).unwrap()
    }

    fn set_global_state_property(&self, name: &str, value: &str) {
        self.map().get_style_mut().set_global_state_property(name, &Value::String(value.to_owned())).unwrap();
    }
}

fn ll(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).expect("LatLng")
}

/// `parseFilter(expression)`: `*conversion::convertJSON<Filter>(expression, error)`.
fn parse_filter(expression: &str) -> Filter {
    let json: serde_json::Value = serde_json::from_str(expression).expect("filter JSON");
    let mut error = Error::default();
    convert::<Filter>(&Convertible::from(&json), &mut error).unwrap_or_else(|| panic!("filter: {}", error.message))
}

/// `Filter(expression)`.
fn filter(expression: Box<dyn mln_style::expression::Expression>) -> Filter {
    Filter::new(Some(Arc::from(expression)), None)
}

fn layers(ids: &[&str]) -> Option<Vec<String>> {
    Some(ids.iter().map(|id| id.to_string()).collect())
}

fn rendered(ids: Option<Vec<String>>, filter: Option<Filter>) -> RenderedQueryOptions {
    RenderedQueryOptions::new(ids, filter)
}

// gtest: Query.QueryRenderedFeatures
#[test]
fn query_rendered_features() {
    let test = QueryTest::new();

    // Batch conversion of latLngs to pixels
    let points = test.map().pixels_for_lat_lngs(&[ll(0.0, 0.0), ll(9.0, 9.0)]);
    assert_eq!(2, points.len());
    // Single conversion of latLng to pixel
    let point0 = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));
    assert_near(points[0].x, point0.x, 1e-8);
    assert_near(points[0].y, point0.y, 1e-8);

    let point1 = test.map().pixel_for_lat_lng(&ll(9.0, 9.0));
    assert_near(points[1].x, point1.x, 1e-8);
    assert_near(points[1].y, point1.y, 1e-8);

    let features1 = test.query_rendered_features(&point0, &RenderedQueryOptions::default());
    assert_eq!(features1.len(), 4);

    let features2 = test.query_rendered_features(&point1, &RenderedQueryOptions::default());
    assert_eq!(features2.len(), 0);
}

// gtest: Query.QueryRenderedFeaturesFilterLayer
#[test]
fn query_rendered_features_filter_layer() {
    let test = QueryTest::new();

    let zz = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));

    let features1 = test.query_rendered_features(&zz, &rendered(layers(&["layer1"]), None));
    assert_eq!(features1.len(), 1);

    let features2 = test.query_rendered_features(&zz, &rendered(layers(&["layer1", "layer2"]), None));
    assert_eq!(features2.len(), 2);

    let features3 = test.query_rendered_features(&zz, &rendered(layers(&["foobar"]), None));
    assert_eq!(features3.len(), 0);

    let features4 = test.query_rendered_features(&zz, &rendered(layers(&["foobar", "layer3"]), None));
    assert_eq!(features4.len(), 1);
}

// gtest: Query.QueryRenderedFeaturesLayer
#[test]
fn query_rendered_features_layer() {
    let test = QueryTest::new();

    let zz = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));

    let features = test.query_rendered_features(&zz, &RenderedQueryOptions::default());
    let mut layers: Vec<&str> = features.iter().map(|feature| feature.layer.as_str()).collect();
    layers.sort_unstable();
    assert_eq!(layers, ["layer1", "layer2", "layer3", "layer4"]);

    let source_features = test.query_source_features("source3", &SourceQueryOptions::default());
    assert!(!source_features.is_empty());
    assert_eq!(source_features[0].layer, "");
}

// gtest: Query.QueryRenderedFeaturesFilter
#[test]
fn query_rendered_features_filter() {
    let test = QueryTest::new();
    let zz = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));

    let eq_filter = filter(eq(get_str("key1"), literal_str("value1")));
    let features1 = test.query_rendered_features(&zz, &rendered(None, Some(eq_filter)));
    assert_eq!(features1.len(), 1);

    let id_not_eq_filter = filter(ne(id(), literal_str("feature1")));
    let features2 = test.query_rendered_features(&zz, &rendered(layers(&["layer4"]), Some(id_not_eq_filter)));
    assert_eq!(features2.len(), 0);

    let gt_filter = filter(gt(number(get_str("key2"), None), literal(ExpressionValue::Number(1.0))));
    let features3 = test.query_rendered_features(&zz, &rendered(None, Some(gt_filter)));
    assert_eq!(features3.len(), 1);
}

// gtest: Query.GlobalStateChangeRelayoutsSourceFilter
#[test]
fn global_state_change_relayouts_source_filter() {
    let test = QueryTest::new();
    {
        let mut map = test.map();
        let mut style = map.get_style_mut();
        let mut layer = style.get_layer_mut("layer4").expect("layer4");
        layer.set_filter(&parse_filter(r#"["==", ["get", "key1"], ["global-state", "filterValue"]]"#));
    }

    let point = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));
    let options = rendered(layers(&["layer4"]), None);

    test.set_global_state_property("filterValue", "value1");
    test.render();
    assert_eq!(1, test.query_rendered_features(&point, &options).len());

    test.set_global_state_property("filterValue", "other");
    test.render();
    assert!(test.query_rendered_features(&point, &options).is_empty());

    test.set_global_state_property("filterValue", "value1");
    test.render();
    assert_eq!(1, test.query_rendered_features(&point, &options).len());
}

// gtest: Query.QueryFiltersUseGlobalState
#[test]
fn query_filters_use_global_state() {
    let test = QueryTest::new();
    let filter = parse_filter(r#"["==", ["get", "key1"], ["global-state", "queryValue"]]"#);
    let point = test.map().pixel_for_lat_lng(&ll(0.0, 0.0));
    let rendered_options = rendered(layers(&["layer4"]), Some(filter.clone()));
    let source_options = SourceQueryOptions::new(None, Some(filter));

    test.set_global_state_property("queryValue", "value1");
    test.render();
    assert_eq!(1, test.query_rendered_features(&point, &rendered_options).len());
    assert_eq!(1, test.query_source_features("source4", &source_options).len());

    test.set_global_state_property("queryValue", "other");
    test.render();
    assert!(test.query_rendered_features(&point, &rendered_options).is_empty());
    assert!(test.query_source_features("source4", &source_options).is_empty());
}

// gtest: Query.QueryRenderedFeaturesLineOffsetEndpoint
#[test]
fn query_rendered_features_line_offset_endpoint() {
    for offset in [22.0f64, -22.0] {
        let test = QueryTest::new();
        // std::to_string(double): "%f".
        test.map().get_style_mut().load_json(&format!(
            r#"{{"version":8,"sources":{{"source":{{"type":"geojson","data":{{
                "type":"LineString","coordinates":[[1.0025,1],[1.0035,1]]
            }}}}}},"layers":[{{"id":"layer","type":"line","source":"source","paint":{{
                "line-width":6,"line-offset":{offset:.6}}}}}]}}"#
        ));
        test.map().jump_to(&CameraOptions::default().with_center(Some(ll(1.0, 1.003))).with_zoom(Some(16.5)));
        let rendered_result = test.render();
        let endpoint = test.map().pixel_for_lat_lng(&ll(1.0, 1.0035));
        // Stay inside the butt cap, away from antialiased edges.
        let inside = ScreenCoordinate::new(endpoint.x - 5.0, endpoint.y + offset);
        let outside = ScreenCoordinate::new(endpoint.x - 5.0, endpoint.y);
        let alpha_at = |point: &ScreenCoordinate| {
            let image = &rendered_result.image;
            image.data[4 * ((point.y as u32 * image.size.width + point.x as u32) as usize) + 3]
        };
        assert!(alpha_at(&inside) > 0, "offset {offset}");
        assert_eq!(alpha_at(&outside), 0, "offset {offset}");
        assert_eq!(
            test.query_rendered_features(&inside, &rendered(layers(&["layer"]), None)).len(),
            1,
            "offset {offset}"
        );
        assert!(
            test.query_rendered_features(&outside, &rendered(layers(&["layer"]), None)).is_empty(),
            "offset {offset}"
        );
    }
}

// gtest: Query.QueryRenderedFeaturesCompositePaintZoom
#[test]
fn query_rendered_features_composite_paint_zoom() {
    // (property, inside, outside)
    let cases: [(&str, f64, f64); 4] = [
        ("line-width", 8.0, 14.0),
        ("line-gap-width", 14.0, 20.0),
        ("circle-radius", 15.0, 26.0),
        ("circle-stroke-width", 24.0, 32.0),
    ];

    // Exercise fractional zoom both below and above the default GeoJSON maxzoom (18).
    for zoom in [16.5f64, 20.5] {
        for (property, inside_offset, outside_offset) in cases {
            let trace = format!("{property} at zoom {zoom}");
            let test = QueryTest::new();
            let circle = property.starts_with("circle-");
            let geometry = if circle {
                r#"{"type":"Point","coordinates":[1.003,1]}"#
            } else {
                r#"{"type":"LineString","coordinates":[[0.993,1],[1.013,1]]}"#
            };
            let paint = if property == "circle-stroke-width" {
                r#""circle-radius":6,"#
            } else if circle || property == "line-width" {
                ""
            } else {
                r#""line-width":6,"#
            };
            let layer_type = if circle { "circle" } else { "line" };
            // std::to_string(int).
            let lower_zoom = zoom as i32;
            let upper_zoom = zoom as i32 + 1;
            test.map().get_style_mut().load_json(&format!(
                r#"{{"version":8,"sources":{{"source":{{"type":"geojson","data":{{
                    "type":"Feature","properties":{{"small":4,"large":40}},"geometry":{geometry}
                }}}}}},"layers":[{{"id":"layer","type":"{layer_type}","source":"source","paint":{{{paint}
                    "{property}":["interpolate",["linear"],["zoom"],{lower_zoom},["get","small"],
                    {upper_zoom},["get","large"]]}}}}]}}"#
            ));
            test.map().jump_to(&CameraOptions::default().with_center(Some(ll(1.0, 1.003))).with_zoom(Some(zoom)));
            let rendered_result = test.render();
            let center = test.map().pixel_for_lat_lng(&ll(1.0, 1.003));
            let inside = ScreenCoordinate::new(center.x, center.y + inside_offset);
            let outside = ScreenCoordinate::new(center.x, center.y + outside_offset);

            // At the camera zoom the composite value is 22; at canonical tile zoom it is 4.
            // Check actual painted pixels as well as query results, away from antialiased edges.
            let alpha_at = |point: &ScreenCoordinate| {
                let image = &rendered_result.image;
                image.data[4 * ((point.y as u32 * image.size.width + point.x as u32) as usize) + 3]
            };
            assert!(alpha_at(&inside) > 0, "{trace}");
            assert_eq!(alpha_at(&outside), 0, "{trace}");
            assert_eq!(test.query_rendered_features(&inside, &rendered(layers(&["layer"]), None)).len(), 1, "{trace}");
            assert!(test.query_rendered_features(&outside, &rendered(layers(&["layer"]), None)).is_empty(), "{trace}");
        }
    }
}

// gtest: Query.QueryRenderedFeaturesFilterUsesOverscaledZoom
#[test]
fn query_rendered_features_filter_uses_overscaled_zoom() {
    for camera_zoom in [16.5f64, 20.5] {
        let test = QueryTest::new();
        {
            let mut map = test.map();
            let mut style = map.get_style_mut();
            // Keep the point away from tile boundaries so source queries return one copy.
            let source = style.get_source("source1").expect("source1");
            let source = source.as_any().downcast_ref::<GeoJSONSource>().expect("GeoJSONSource");
            source.set_geojson(&GeoJSON::Geometry(Geometry::Point(Point::new(1.003, 1.003)))).unwrap();
            style.add_layer(Box::new(CircleLayer::new("circle_layer", "source1")), None).unwrap();
        }
        // std::to_string(int).
        let tile_zoom = camera_zoom as i32;
        let equal_zoom = parse_filter(&format!(r#"["==", ["zoom"], {tile_zoom}]"#));
        let lower_zoom = parse_filter(&format!(r#"["<", ["zoom"], {tile_zoom}]"#));
        let higher_zoom = parse_filter(&format!(r#"[">", ["zoom"], {tile_zoom}]"#));

        // Layer filters and query filters should agree on the integer bucket zoom.
        // Cover both the collision-index symbol path and the geometry-index circle path.
        for layer_id in ["layer1", "circle_layer"] {
            let mut map = test.map();
            let mut style = map.get_style_mut();
            style.get_layer_mut(layer_id).expect("layer").set_filter(&equal_zoom);
        }
        test.map().jump_to(&CameraOptions::default().with_center(Some(ll(1.003, 1.003))).with_zoom(Some(camera_zoom)));
        test.render();
        let point = test.map().pixel_for_lat_lng(&ll(1.003, 1.003));
        for layer_id in ["layer1", "circle_layer"] {
            let trace = format!("{layer_id} at zoom {camera_zoom}");
            assert_eq!(test.query_rendered_features(&point, &rendered(layers(&[layer_id]), None)).len(), 1, "{trace}");
            assert_eq!(
                test.query_rendered_features(&point, &rendered(layers(&[layer_id]), Some(equal_zoom.clone()))).len(),
                1,
                "{trace}"
            );
            // Reject canonical zoom when overzoomed, and fractional camera zoom at either zoom.
            assert!(
                test.query_rendered_features(&point, &rendered(layers(&[layer_id]), Some(lower_zoom.clone())))
                    .is_empty(),
                "{trace}"
            );
            assert!(
                test.query_rendered_features(&point, &rendered(layers(&[layer_id]), Some(higher_zoom.clone())))
                    .is_empty(),
                "{trace}"
            );
        }
        assert_eq!(
            test.query_source_features("source1", &SourceQueryOptions::new(None, Some(equal_zoom.clone()))).len(),
            1,
            "zoom {camera_zoom}"
        );
    }
}

// gtest: Query.QuerySourceFeatures
#[test]
fn query_source_features() {
    let test = QueryTest::new();

    let features1 = test.query_source_features("source3", &SourceQueryOptions::default());
    assert_eq!(features1.len(), 1);
}

// gtest: Query.QuerySourceFeatureStates
#[test]
fn query_source_feature_states() {
    let test = QueryTest::new();

    let mut new_state = FeatureState::new();
    new_state.insert("hover".into(), Value::Bool(true));
    new_state.insert("radius".into(), Value::Uint(20));
    test.0.frontend.with_renderer(|renderer| renderer.set_feature_state("source1", None, "feature1", &new_state));

    // C++ fills an out-parameter; the Rust getter returns the state.
    let states = test.0.frontend.with_renderer(|renderer| renderer.get_feature_state("source1", None, "feature1"));
    assert_eq!(states.len(), 2);
    assert_eq!(states["hover"], Value::Bool(true));
    assert_eq!(states["radius"], Value::Uint(20));
    assert_eq!(new_state, states);
}

// gtest: Query.RemoveSourceFeatureState
#[test]
fn remove_source_feature_state() {
    let test = QueryTest::new();
    let frontend = test.0.frontend.clone();

    // Set two state values on a feature. Updates are visible immediately,
    // without waiting for a render pass.
    let mut new_state = FeatureState::new();
    new_state.insert("hover".into(), Value::Bool(true));
    new_state.insert("radius".into(), Value::Uint(20));
    frontend.with_renderer(|renderer| renderer.set_feature_state("source1", None, "feature1", &new_state));

    // Read back (C++: the out-parameter overload).
    let after_set = frontend.with_renderer(|renderer| renderer.get_feature_state("source1", None, "feature1"));
    assert_eq!(after_set, new_state);

    // Remove a single key. Unlike updates, removals are only reflected once a
    // render pass has coalesced the pending changes into the source state, so
    // render before reading back.
    frontend.with_renderer(|renderer| renderer.remove_feature_state("source1", None, Some("feature1"), Some("hover")));
    test.render();

    // Read back (C++: the return-value overload).
    let after_key_removal = frontend.with_renderer(|renderer| renderer.get_feature_state("source1", None, "feature1"));
    assert_eq!(after_key_removal.len(), 1);
    assert!(!after_key_removal.contains_key("hover"));
    assert_eq!(after_key_removal["radius"], Value::Uint(20));

    // Removing the whole feature (no state key) clears any remaining state.
    frontend.with_renderer(|renderer| renderer.remove_feature_state("source1", None, Some("feature1"), None));
    test.render();

    // Read back (C++: the out-parameter overload again).
    let after_feature_removal =
        frontend.with_renderer(|renderer| renderer.get_feature_state("source1", None, "feature1"));
    assert!(after_feature_removal.is_empty());
}

// gtest: Query.QuerySourceFeaturesOptionValidation
#[test]
fn query_source_features_option_validation() {
    let test = QueryTest::new();

    // GeoJSONSource, doesn't require a layer id
    let mut features = test.query_source_features("source3", &SourceQueryOptions::default());
    assert_eq!(features.len(), 1);

    // VectorSource, requires a layer id
    features = test.query_source_features("source5", &SourceQueryOptions::default());
    assert_eq!(features.len(), 0);

    // RasterSource, not supported
    features = test.query_source_features("source6", &SourceQueryOptions::default());
    assert_eq!(features.len(), 0);
}

/// One MVT tile with two layers, "points" and "labels", each holding one
/// point feature at the tile's center: ID 1 in "points" and ID 2 in
/// "labels".
const TWO_LAYER_TILE: &[u8] = &[
    0x1a, 0x1a, 0x78, 0x02, 0x0a, 0x06, b'p', b'o', b'i', b'n', b't', b's', 0x12, 0x0b, 0x08, 0x01, 0x18, 0x01, 0x22,
    0x05, 0x09, 0x80, 0x20, 0x80, 0x20, 0x28, 0x80, 0x20, 0x1a, 0x1a, 0x78, 0x02, 0x0a, 0x06, b'l', b'a', b'b', b'e',
    b'l', b's', 0x12, 0x0b, 0x08, 0x02, 0x18, 0x01, 0x22, 0x05, 0x09, 0x80, 0x20, 0x80, 0x20, 0x28, 0x80, 0x20,
];

// Not a C++ test: ffi patch 0048 has none. Ports the source-layer checks of
// maplibre-native-ffi's `source_queries_read_the_named_source_layers` ABI
// test.
#[test]
fn query_source_features_source_layer() {
    let test = MapTest::new();
    test.file_source.set_response(Kind::Tile, |_| {
        let mut response = mln_storage::Response::new();
        response.data = Some(Arc::new(TWO_LAYER_TILE.to_vec()));
        Some(response)
    });
    test.load_json(
        r#"{
          "version": 8,
          "sources": { "tiles": { "type": "vector", "tiles": ["http://example.com/{z}/{x}/{y}.pbf"], "maxzoom": 0 } },
          "layers": [{ "id": "dots", "type": "circle", "source": "tiles", "source-layer": "points" }]
        }"#,
    );
    test.render();

    let query = |source_layers: &[&str], filter: Option<Filter>| -> Vec<(u64, String)> {
        let options = SourceQueryOptions::new(layers(source_layers), filter);
        let features =
            test.frontend.with_renderer(|renderer| renderer.query_source_features("tiles", &options)).unwrap();
        let mut hits: Vec<(u64, String)> = features
            .into_iter()
            .map(|feature| {
                assert_eq!(feature.layer, "");
                let feature::Identifier::Uint(id) = feature.id else { panic!("feature ID: {:?}", feature.id) };
                (id, feature.source_layer)
            })
            .collect();
        hits.sort_unstable();
        hits
    };

    assert_eq!(query(&["labels"], None), [(2, "labels".to_owned())]);
    assert_eq!(query(&["points", "labels"], None), [(1, "points".to_owned()), (2, "labels".to_owned())]);
    assert_eq!(query(&["points", "labels"], Some(parse_filter(r#"["==", ["id"], 1]"#))), [(1, "points".to_owned())]);
}

// gtest: Query.QuerySourceFeaturesFilter
#[test]
fn query_source_features_filter() {
    let test = QueryTest::new();

    let eq_filter = filter(eq(get_str("key1"), literal_str("value1")));
    let features1 = test.query_source_features("source4", &SourceQueryOptions::new(None, Some(eq_filter)));
    assert_eq!(features1.len(), 1);

    let id_not_eq_filter = filter(ne(id(), literal_str("feature1")));
    let features2 = test.query_source_features("source4", &SourceQueryOptions::new(None, Some(id_not_eq_filter)));
    assert_eq!(features2.len(), 0);

    let gt_filter = filter(gt(number(get_str("key2"), None), literal(ExpressionValue::Number(1.0))));
    let features3 = test.query_source_features("source4", &SourceQueryOptions::new(None, Some(gt_filter)));
    assert_eq!(features3.len(), 1);
}

/// `frontend.getRenderer()->queryFeatureExtensions(sourceID, feature,
/// extension, extensionField, args)`.
fn query_feature_extensions(
    test: &QueryTest,
    source_id: &str,
    feature: &Feature,
    extension: &str,
    extension_field: &str,
    args: Option<&PropertyMap>,
) -> FeatureExtensionValue {
    test.0
        .frontend
        .with_renderer(|renderer| {
            renderer.query_feature_extensions(source_id, feature, extension, extension_field, args)
        })
        .expect("queryFeatureExtensions")
}

fn feature_collection(value: FeatureExtensionValue) -> Vec<feature::Feature<f64>> {
    match value {
        FeatureExtensionValue::FeatureCollection(features) => features,
        other => panic!("not a FeatureCollection: {other:?}"),
    }
}

/// `getTopClusterFeature(test)`: the top cluster of
/// test/fixtures/supercluster/places.json, clustered at z0.
fn get_top_cluster_feature(test: &QueryTest) -> Vec<Feature> {
    test.0.file_source.set_response(Kind::Source, |resource| {
        assert_eq!("http://url", resource.url);
        Some(file_response("test/fixtures/supercluster/places.json"))
    });

    let coordinate = ll(0.0, 0.0);
    let options = GeoJSONOptions { cluster: true, ..GeoJSONOptions::default() };
    let source = GeoJSONSource::new("cluster_source", Arc::new(options));
    source.set_url("http://url");
    source.load_description(&*test.0.file_source);

    let mut cluster_layer = SymbolLayer::new("cluster_layer", "cluster_source");
    cluster_layer.set_icon_image(&PropertyValue::Constant(mln_style::expression::Image::new("test-icon")));
    cluster_layer.set_icon_size(&PropertyValue::Constant(12.0));

    test.map().jump_to(&CameraOptions::default().with_center(Some(coordinate)).with_zoom(Some(0.0)));
    test.map().get_style_mut().add_source(Box::new(source)).unwrap();
    test.map().get_style_mut().add_layer(Box::new(cluster_layer), None).unwrap();
    test.0.run_loop.run_once();
    test.render();

    let screen_coordinate = test.map().pixel_for_lat_lng(&coordinate);
    let query_options = RenderedQueryOptions::new(Some(vec!["cluster_layer".to_owned()]), None);
    test.query_rendered_features(&screen_coordinate, &query_options)
}

fn uint_property(feature: &feature::Feature<f64>, name: &str) -> Option<u64> {
    match feature.properties.get(name) {
        Some(Value::Uint(value)) => Some(*value),
        _ => None,
    }
}

fn string_property<'a>(feature: &'a feature::Feature<f64>, name: &str) -> Option<&'a str> {
    match feature.properties.get(name) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

// gtest: Query.QueryFeatureExtensionsInvalidExtension
#[test]
fn query_feature_extensions_invalid_extension() {
    let test = QueryTest::new();

    let unknown_ext = query_feature_extensions(&test, "source4", &Feature::default(), "unknown", "children", None);
    assert_eq!(unknown_ext, FeatureExtensionValue::Value(Value::Null));
}

// gtest: Query.QueryFeatureExtensionsSuperclusterChildren
#[test]
fn query_feature_extensions_supercluster_children() {
    let test = QueryTest::new();
    let top_cluster_feature = get_top_cluster_feature(&test);

    assert_eq!(top_cluster_feature.len(), 1);
    let feature_props = &top_cluster_feature[0].properties;
    assert!(feature_props.contains_key("cluster_id"));
    assert!(feature_props.contains_key("cluster"));

    let query_children =
        query_feature_extensions(&test, "cluster_source", &top_cluster_feature[0], "supercluster", "children", None);
    let children = feature_collection(query_children);
    assert_eq!(children.len(), 4);

    // Compare results produced by supercluster with default clustering options.
    let cluster_ids: Vec<_> = children.iter().map(|c| uint_property(c, "cluster_id")).collect();
    assert_eq!(cluster_ids, [Some(2), Some(34), Some(258), Some(2466)]);
    let point_counts: Vec<_> = children.iter().map(|c| uint_property(c, "point_count")).collect();
    assert_eq!(point_counts, [Some(7), Some(16), Some(7), Some(2)]);
}

// gtest: Query.QueryFeatureExtensionsSuperclusterExpansionZoom
#[test]
fn query_feature_extensions_supercluster_expansion_zoom() {
    let test = QueryTest::new();
    let top_cluster_feature = get_top_cluster_feature(&test);
    assert_eq!(top_cluster_feature.len(), 1);

    let children = feature_collection(query_feature_extensions(
        &test,
        "cluster_source",
        &top_cluster_feature[0],
        "supercluster",
        "children",
        None,
    ));

    let query_expansion_zoom1 = query_feature_extensions(
        &test,
        "cluster_source",
        &top_cluster_feature[0],
        "supercluster",
        "expansion-zoom",
        None,
    );
    let query_expansion_zoom2 = query_feature_extensions(
        &test,
        "cluster_source",
        &Feature::from(children[3].clone()),
        "supercluster",
        "expansion-zoom",
        None,
    );
    assert_eq!(query_expansion_zoom1, FeatureExtensionValue::Value(Value::Uint(1)));
    assert_eq!(query_expansion_zoom2, FeatureExtensionValue::Value(Value::Uint(3)));
}

// gtest: Query.QueryFeatureExtensionsSuperclusterLeaves
#[test]
fn query_feature_extensions_supercluster_leaves() {
    let test = QueryTest::new();
    let top_cluster_feature = get_top_cluster_feature(&test);
    assert_eq!(top_cluster_feature.len(), 1);
    let leaves = |args: Option<&PropertyMap>| {
        feature_collection(query_feature_extensions(
            &test,
            "cluster_source",
            &top_cluster_feature[0],
            "supercluster",
            "leaves",
            args,
        ))
    };
    let names = |features: &[feature::Feature<f64>]| -> Vec<String> {
        features.iter().map(|f| string_property(f, "name").unwrap_or_default().to_owned()).collect()
    };

    // Get leaves for cluster 1, with default limit 10, offset 0.
    assert_eq!(leaves(None).len(), 10);

    // Get leaves for cluster 1, with limit 3, offset 0.
    let limit_opts = PropertyMap::from_iter([("limit".to_owned(), Value::Uint(3))]);
    let limit_leaves3 = leaves(Some(&limit_opts));
    assert_eq!(names(&limit_leaves3), ["Niagara Falls", "Cape May", "Cape Fear"]);

    // Get leaves for cluster 1, with limit 3, offset 3.
    let offset_opts =
        PropertyMap::from_iter([("limit".to_owned(), Value::Uint(3)), ("offset".to_owned(), Value::Uint(3))]);
    let offset_leaves3 = leaves(Some(&offset_opts));
    assert_eq!(names(&offset_leaves3), ["Cape Hatteras", "Cape Sable", "Cape Cod"]);
}

#[test]
fn query_feature_extensions_unknown_cluster() {
    // A cluster ID the source doesn't have is the error C++ throws.
    let test = QueryTest::new();
    let mut top_cluster_feature = get_top_cluster_feature(&test);
    assert_eq!(top_cluster_feature.len(), 1);
    top_cluster_feature[0].properties.insert("cluster_id".to_owned(), Value::Uint(7));

    let result = test.0.frontend.with_renderer(|renderer| {
        renderer.query_feature_extensions("cluster_source", &top_cluster_feature[0], "supercluster", "children", None)
    });
    let error = result.expect_err("unknown cluster");
    assert!(error.downcast_ref::<RuntimeError>().is_some(), "{error}");
}
