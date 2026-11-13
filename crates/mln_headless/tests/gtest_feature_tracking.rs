// Ports test/renderer/feature_tracking.test.cpp: the features a map with
// `MapOptions::capture_rendered_features` reports as rendered in the last
// frame, and their bounds in normalized device coordinates.

mod support;

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_map::{CameraOptions, FeatureInfo, Map, MapDebugOptions, MapMode, MapOptions, NDCBound};
use mln_storage::Kind;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::PropertyValue;
use mln_style::expression::Value as ExpressionValue;
use mln_style::expression::dsl::{eq, format, get, interpolate, linear, literal, literal_str, zoom};
use mln_style::filter::Filter;
use mln_style::layer::Layer;
use mln_style::layers::background_layer::BackgroundLayer;
use mln_style::layers::circle_layer::CircleLayer;
use mln_style::layers::fill_extrusion_layer::FillExtrusionLayer;
use mln_style::layers::fill_layer::FillLayer;
use mln_style::layers::line_layer::LineLayer;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style::projection::Projection;
use mln_style::property_expression::PropertyExpression;
use mln_style::sources::geojson_source::{GeoJSONOptions, GeoJSONSource};
use mln_style::style_impl::LayerRefMut;
use mln_style_types::{
    AlignmentType, CirclePitchScaleType, ProjectionDefinition, ProjectionType, SymbolAnchorType, SymbolPlacementType,
    VisibilityType,
};
use mln_util::Color;
use mln_util::geo::LatLng;
use mln_util::image::PremultipliedImage;
use support::*;

const SOURCE_NAME: &str = "source";
const MARKER_NAME: &str = "marker";
const LINE_LAYER_NAME: &str = "line";
const FILL_LAYER_NAME: &str = "fill";
const CIRCLE_LAYER_NAME: &str = "circle";
const SYMBOL_LAYER_NAME: &str = "symbol";

const FIXTURE_BASE: &str = "test/fixtures/";
const GLYPH_PATH: &str = "test/fixtures/resources/glyphs.pbf";
const MARKER_PATH: &str = "test/fixtures/sprites/default_marker.png";

/// `FeatureTrackingTest::defaultGeometry()`.
const DEFAULT_GEOMETRY: &str = r#"{"type": "FeatureCollection", "features": [
    {"type": "Feature", "id": "pt0", "properties": {"name": "0", "opacity": 0.5},
     "geometry": {"type": "Point", "coordinates": [0.01, 0.02]}},
    {"type": "Feature", "id": "pt0_overlap", "properties": {"name": "0"},
     "geometry": {"type": "Point", "coordinates": [0.012, 0.022]}},
    {"type": "Feature", "id": "pt1", "properties": {"name": "1", "opacity": 0.0},
     "geometry": {"type": "Point", "coordinates": [0.03, -0.04]}},
    {"type": "Feature", "id": "pt2", "properties": {"name": "2"},
     "geometry": {"type": "Point", "coordinates": [0.1, 0.2]}},
    {"type": "Feature", "id": "l1", "properties": {"name": "3"},
     "geometry": {"type": "LineString", "coordinates": [[0.01, 0.02], [0.03, -0.04]]}},
    {"type": "Feature", "id": "poly0", "properties": {"name": "4"},
     "geometry": {"type": "Polygon",
                  "coordinates": [[[0.01, 0.02], [0.03, -0.04], [-0.07, -0.08], [-0.06, 0.04]]]}}
]}"#;

fn constant<T>(value: T) -> PropertyValue<T> {
    PropertyValue::Constant(value)
}

/// `FeatureTrackingTest`.
struct FeatureTrackingTest {
    map: Map,
    frontend: HeadlessFrontend,
    _run_loop: RunLoop,
}

impl FeatureTrackingTest {
    fn new(camera: CameraOptions) -> Self {
        Self::with_geojson(DEFAULT_GEOMETRY, camera)
    }

    fn with_geojson(geojson: &str, camera: CameraOptions) -> Self {
        let run_loop = RunLoop::new();
        let frontend = HeadlessFrontend::with_pixel_ratio(1.0).expect("a headless frontend");
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        file_source.set_response(Kind::Glyphs, |_| Some(file_response(GLYPH_PATH)));
        let mut map = Map::with_file_source(
            file_source,
            MapOptions {
                map_mode: MapMode::Static,
                size: frontend.get_size(),
                capture_rendered_features: true,
                ..Default::default()
            },
        )
        .expect("map");
        frontend.attach(&map);

        let source = GeoJSONSource::with_default_options(SOURCE_NAME);
        source.set_geojson(&geojson::parse(geojson).expect("GeoJSON")).expect("set GeoJSON");

        let mut background_layer = BackgroundLayer::new("background");
        background_layer.set_background_color(&constant(Color::new(1.0, 1.0, 1.0, 1.0)));

        let mut fill_layer = FillLayer::new(FILL_LAYER_NAME, SOURCE_NAME);
        fill_layer.set_visibility(VisibilityType::None);
        fill_layer.set_fill_color(&constant(Color::new(0.0, 0.0, 1.0, 1.0)));

        let mut line_layer = LineLayer::new(LINE_LAYER_NAME, SOURCE_NAME);
        line_layer.set_visibility(VisibilityType::None);
        line_layer.set_line_color(&constant(Color::new(0.0, 0.0, 0.0, 1.0)));
        line_layer.set_line_width(&constant(2.0));

        let mut circle_layer = CircleLayer::new(CIRCLE_LAYER_NAME, SOURCE_NAME);
        circle_layer.set_visibility(VisibilityType::None);
        circle_layer.set_circle_color(&constant(Color::new(0.0, 0.0, 0.0, 1.0)));
        circle_layer.set_circle_opacity(&constant(0.0));
        circle_layer.set_circle_radius(&constant(30.0));
        circle_layer.set_circle_stroke_color(&constant(Color::new(0.0, 0.0, 0.0, 1.0)));
        circle_layer.set_circle_stroke_width(&constant(2.0));

        let mut symbol_layer = SymbolLayer::new(SYMBOL_LAYER_NAME, SOURCE_NAME);
        symbol_layer.set_visibility(VisibilityType::None);
        symbol_layer.set_icon_allow_overlap(&constant(false));
        symbol_layer.set_symbol_placement(&constant(SymbolPlacementType::Point));
        symbol_layer.set_icon_image(&constant(mln_style::expression::Image::new(MARKER_NAME)));
        symbol_layer.set_text_field(&PropertyValue::Expression(PropertyExpression::from_box(
            format(get(literal_str("name"))),
            None,
        )));
        symbol_layer.set_text_size(&constant(24.0));
        symbol_layer.set_text_color(&constant(Color::new(0.0, 0.0, 0.0, 1.0)));
        symbol_layer.set_text_anchor(&constant(SymbolAnchorType::Right));
        symbol_layer.set_text_offset(&constant([1.0, 0.0]));

        let image = mln_style::image::Impl::new(
            MARKER_NAME.to_owned(),
            decode_image(MARKER_PATH),
            1.0,
            false,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .expect("style::Image");

        {
            let style = map.style_mut();
            style.load_json(&read_file(&format!("{FIXTURE_BASE}api/empty.json")));
            style.add_source(Box::new(source)).expect("add source");
            style.add_layer(Box::new(background_layer), None).expect("add layer");
            style.add_layer(Box::new(fill_layer), None).expect("add layer");
            style.add_layer(Box::new(line_layer), None).expect("add layer");
            style.add_layer(Box::new(circle_layer), None).expect("add layer");
            style.add_layer(Box::new(symbol_layer), None).expect("add layer");
            style.add_image(Arc::new(image));
        }

        map.set_debug(MapDebugOptions::TILE_BORDERS);
        map.jump_to(&camera);

        FeatureTrackingTest { map, frontend, _run_loop: run_loop }
    }

    /// `run(images, imageThreshold, pixelThreshold = 0.1)`: renders, and
    /// compares with `test/fixtures/feature_tracking`/<image>/expected.png.
    fn run(&mut self, image: Option<&str>, image_threshold: f64) -> RenderResult {
        let result = self.frontend.render(&mut self.map).expect("render");
        if let Some(image) = image {
            check_image(&format!("{FIXTURE_BASE}feature_tracking/{image}"), &result.image, image_threshold, 0.1);
        }
        result
    }

    fn with_layer(&mut self, id: &str, f: impl FnOnce(&mut LayerRefMut<'_>)) {
        let style = self.map.style_mut();
        f(&mut style.get_layer_mut(id).expect("layer"));
    }

    fn with_symbol_layer(&mut self, f: impl FnOnce(&mut SymbolLayer)) {
        self.with_layer(SYMBOL_LAYER_NAME, |layer| f(layer.downcast_mut::<SymbolLayer>().expect("SymbolLayer")));
    }

    fn with_circle_layer(&mut self, f: impl FnOnce(&mut CircleLayer)) {
        self.with_layer(CIRCLE_LAYER_NAME, |layer| f(layer.downcast_mut::<CircleLayer>().expect("CircleLayer")));
    }

    fn show(&mut self, id: &str) {
        self.with_layer(id, |layer| layer.set_visibility(VisibilityType::Visible));
    }

    fn count(&self, feature_id: Option<&str>, layer_id: Option<&str>, source_id: Option<&str>) -> usize {
        self.map.get_rendered_feature_count(feature_id, layer_id, source_id)
    }

    /// Checks `check` on each matching feature, and that there is one.
    fn check_features(
        &self,
        feature_id: Option<&str>,
        layer_id: Option<&str>,
        source_id: Option<&str>,
        check: impl Fn(&FeatureInfo),
    ) {
        let mut checked = 0;
        self.map.get_rendered_features(feature_id, layer_id, source_id, |_, info| {
            check(info);
            checked += 1;
            true
        });
        assert!(checked > 0, "no rendered feature {feature_id:?} in {layer_id:?}");
    }
}

fn camera(zoom_level: f64) -> CameraOptions {
    CameraOptions { zoom: Some(zoom_level), ..Default::default() }
}

// gtest: FeatureTracking.ApiQuery
#[test]
fn api_query() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.run(Some("ApiQuery"), 0.0);

    // Positive and negative results on each filter criterion
    let source_x = format!("{SOURCE_NAME}x");
    assert_eq!(3, test.count(None, None, None));
    assert_eq!(1, test.count(Some("pt0"), None, None));
    assert_eq!(0, test.count(Some("pt2"), None, None));
    assert_eq!(1, test.count(Some("pt0"), Some("symbol"), None));
    assert_eq!(0, test.count(Some("pt0"), Some("symbol0"), None));
    assert_eq!(3, test.count(None, Some("symbol"), None));
    assert_eq!(0, test.count(None, Some("symbol0"), None));
    assert_eq!(3, test.count(None, None, Some(SOURCE_NAME)));
    assert_eq!(0, test.count(None, None, Some(&source_x)));
    assert_eq!(1, test.count(Some("pt0"), Some("symbol"), Some(SOURCE_NAME)));
    assert_eq!(0, test.count(Some("pt0"), Some("symbol"), Some(&source_x)));
    assert!(test.map.is_feature_rendered(Some("pt0"), None, None));
    assert!(!test.map.is_feature_rendered(Some("pt2"), None, None));
}

// gtest: FeatureTracking.NDCBoundSymbol
#[test]
fn ndc_bound_symbol() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.show(LINE_LAYER_NAME);
    test.run(Some("NDCBoundSymbol"), 0.0);

    assert_eq!(1, test.count(Some("pt0"), None, None));
    test.check_features(Some("pt0"), None, None, |info| {
        assert!(info.ndc_bound.min_x > 0.0, "{info:?}");
        assert!(info.ndc_bound.max_x < 0.5, "{info:?}");
        assert!(info.ndc_bound.min_y > 0.0, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.5, "{info:?}");
    });

    assert_eq!(1, test.count(Some("poly0"), Some(SYMBOL_LAYER_NAME), None));
    test.check_features(Some("poly0"), Some(SYMBOL_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.max_x < 0.0, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.1, "{info:?}");
    });

    assert_eq!(1, test.count(Some("l1"), Some(LINE_LAYER_NAME), None));
    test.check_features(Some("l1"), Some(LINE_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.min_x > 0.1, "{info:?}");
        assert!(info.ndc_bound.max_x < 0.5, "{info:?}");
        assert!(info.ndc_bound.min_y > -0.5, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.5, "{info:?}");
    });
}

// gtest: FeatureTracking.NDCBoundSymbolOffset
#[test]
fn ndc_bound_symbol_offset() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.with_symbol_layer(|layer| {
        layer.set_icon_translate(&constant([-140.0, -80.0]));
        layer.set_text_translate(&constant([-140.0, -80.0]));
    });
    test.run(Some("NDCBoundSymbolOffset"), 0.0);

    assert_eq!(1, test.count(Some("pt0"), None, None));
    test.check_features(Some("pt0"), None, None, |info| {
        assert!(info.ndc_bound.min_x < -1.0, "{info:?}");
        assert!(info.ndc_bound.max_x < -0.5, "{info:?}");
        assert!(info.ndc_bound.min_y > 0.5, "{info:?}");
        assert!(info.ndc_bound.max_y > 1.0, "{info:?}");
    });
}

/// Shows the symbol layer's pt0 label alone, at a center variable anchor
/// pitched with the map (both translation tests).
fn show_variable_anchor_label(layer: &mut SymbolLayer) {
    layer.set_visibility(VisibilityType::Visible);
    layer.set_icon_image(&PropertyValue::Undefined);
    layer.set_text_variable_anchor(&constant(vec![SymbolAnchorType::Center]));
    layer.set_text_pitch_alignment(&constant(AlignmentType::Map));
}

/// The bottom of pt0's bound in `layer_id`.
fn max_y(test: &FeatureTrackingTest, layer_id: &str) -> f64 {
    assert_eq!(1, test.count(Some("pt0"), Some(layer_id), None));
    let mut y = 0.0;
    test.map.get_rendered_features(Some("pt0"), Some(layer_id), None, |_, info| {
        y = info.ndc_bound.max_y;
        true
    });
    y
}

// A variable-anchor label lays out with its translation already applied;
// the capture follows it once.
// gtest: FeatureTracking.NDCBoundTranslatedVariableAnchor
#[test]
fn ndc_bound_translated_variable_anchor() {
    let capture_at = |translate: [f32; 2]| -> (f64, f64) {
        let mut test = FeatureTrackingTest::new(camera(10.0));
        test.with_symbol_layer(|layer| {
            show_variable_anchor_label(layer);
            layer.set_text_translate(&constant(translate));
        });
        test.run(None, 0.0);
        (max_y(&test, SYMBOL_LAYER_NAME), f64::from(test.frontend.get_size().height))
    };
    let (untranslated, _) = capture_at([0.0, 0.0]);
    let (translated, height) = capture_at([0.0, -50.0]);
    let expected = 2.0 * 50.0 / height;
    assert!((translated - untranslated - expected).abs() <= 1e-3, "{translated} - {untranslated} != {expected}");
}

// A layer drawn from a bucket that another layer places is captured at its
// own translation, where it is drawn.
// gtest: FeatureTracking.NDCBoundSharedBucketAtAnotherTranslation
#[test]
fn ndc_bound_shared_bucket_at_another_translation() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.with_symbol_layer(show_variable_anchor_label);
    {
        let style = test.map.style_mut();
        let mut follower = style.get_layer(SYMBOL_LAYER_NAME).expect("layer").clone_ref("follower");
        follower.downcast_mut::<SymbolLayer>().expect("SymbolLayer").set_text_translate(&constant([0.0, -50.0]));
        style.add_layer(follower, None).expect("add layer");
    }
    test.run(None, 0.0);
    let expected = 2.0 * 50.0 / f64::from(test.frontend.get_size().height);
    let difference = max_y(&test, "follower") - max_y(&test, SYMBOL_LAYER_NAME);
    assert!((difference - expected).abs() <= 1e-3, "{difference} != {expected}");
}

// gtest: FeatureTracking.NDCBoundCircle
#[test]
fn ndc_bound_circle() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(CIRCLE_LAYER_NAME);
    test.with_circle_layer(|layer| layer.set_circle_translate(&constant([-100.0, -20.0])));
    test.show(SYMBOL_LAYER_NAME);
    test.run(Some("NDCBoundCircle"), 0.0);

    assert_eq!(1, test.count(Some("pt0"), Some(CIRCLE_LAYER_NAME), None));
    test.check_features(Some("pt0"), Some(CIRCLE_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.max_x < 0.0, "{info:?}");
        assert!(info.ndc_bound.min_y > 0.0, "{info:?}");
    });
    assert_eq!(1, test.count(Some("pt1"), Some(CIRCLE_LAYER_NAME), None));
    test.check_features(Some("pt1"), Some(CIRCLE_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.max_x < 0.0, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.0, "{info:?}");
    });
}

/// The pitched circle tests: each combination of pitch options projects the
/// vertices differently.
fn pitched_circle_test(image: &str, alignment: AlignmentType, scale: CirclePitchScaleType) -> FeatureTrackingTest {
    let mut test = FeatureTrackingTest::new(CameraOptions { pitch: Some(60.0), ..camera(10.0) });
    test.show(CIRCLE_LAYER_NAME);
    test.with_circle_layer(|layer| {
        layer.set_circle_pitch_alignment(&constant(alignment));
        layer.set_circle_pitch_scale(&constant(scale));
    });
    test.run(Some(image), 0.05);
    assert_eq!(1, test.count(Some("pt2"), Some(CIRCLE_LAYER_NAME), None));
    test
}

// gtest: FeatureTracking.NDCBoundCirclePitch1
#[test]
fn ndc_bound_circle_pitch1() {
    let test = pitched_circle_test("NDCBoundCirclePitch1", AlignmentType::Map, CirclePitchScaleType::Map);
    test.check_features(Some("pt2"), Some(CIRCLE_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.max_x < 0.9, "{info:?}");
        assert!(info.ndc_bound.min_x < 0.6, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.8, "{info:?}");
        assert!(info.ndc_bound.min_y > 0.6, "{info:?}");
    });
}

// gtest: FeatureTracking.NDCBoundCirclePitch2
#[test]
fn ndc_bound_circle_pitch2() {
    pitched_circle_test("NDCBoundCirclePitch2", AlignmentType::Map, CirclePitchScaleType::Viewport);
}

// gtest: FeatureTracking.NDCBoundCirclePitch3
#[test]
fn ndc_bound_circle_pitch3() {
    pitched_circle_test("NDCBoundCirclePitch3", AlignmentType::Viewport, CirclePitchScaleType::Map);
}

// gtest: FeatureTracking.NDCBoundCirclePitch4
#[test]
fn ndc_bound_circle_pitch4() {
    pitched_circle_test("NDCBoundCirclePitch4", AlignmentType::Viewport, CirclePitchScaleType::Viewport);
}

// gtest: FeatureTracking.NDCBoundFill
#[test]
fn ndc_bound_fill() {
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(FILL_LAYER_NAME);
    test.run(Some("NDCBoundFill"), 0.01);

    assert_eq!(1, test.count(Some("poly0"), Some(FILL_LAYER_NAME), None));
    test.check_features(Some("poly0"), Some(FILL_LAYER_NAME), None, |info| {
        assert!(info.ndc_bound.min_x > -1.0, "{info:?}");
        assert!(info.ndc_bound.min_x < -0.75, "{info:?}");
        assert!(info.ndc_bound.max_x > 0.0, "{info:?}");
        assert!(info.ndc_bound.max_x < 0.5, "{info:?}");
        assert!(info.ndc_bound.min_y > -1.0, "{info:?}");
        assert!(info.ndc_bound.min_y < -0.9, "{info:?}");
        assert!(info.ndc_bound.max_y > 0.0, "{info:?}");
        assert!(info.ndc_bound.max_y < 0.5, "{info:?}");
    });
}

// gtest: FeatureTracking.NDCBoundTilt
#[test]
fn ndc_bound_tilt() {
    let geometry = r#"{"type": "FeatureCollection", "features": [
        {"type": "Feature", "id": "pt0", "properties": {"name": "0"},
         "geometry": {"type": "Point", "coordinates": [0.02, 0.25]}}
    ]}"#;

    let mut test = FeatureTrackingTest::with_geojson(geometry, camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.run(None, 0.0);
    assert_eq!(0, test.count(Some("pt0"), None, None)); // out of view

    // tilt and the feature appears
    let pitched = CameraOptions { pitch: Some(60.0), ..test.map.get_camera_options(None) };
    test.map.jump_to(&pitched);
    test.run(Some("NDCBoundTilt"), 0.01);

    test.check_features(Some("pt0"), None, None, |info| {
        assert!(info.ndc_bound.min_x > 0.0, "{info:?}");
        assert!(info.ndc_bound.max_x < 0.5, "{info:?}");
        assert!(info.ndc_bound.min_y > 0.6, "{info:?}");
        assert!(info.ndc_bound.min_y < 0.7, "{info:?}");
        assert!(info.ndc_bound.max_y > 0.9, "{info:?}");
        assert!(info.ndc_bound.max_y < 1.0, "{info:?}");
    });
}

// gtest: FeatureTracking.Overlap
#[test]
fn overlap() {
    // Features hidden by `icon-allow-overlap:false` do not appear in the results
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.run(None, 0.0);
    assert_eq!(0, test.count(Some("pt0_overlap"), None, None));
    test.with_symbol_layer(|layer| {
        layer.set_icon_allow_overlap(&constant(true));
        layer.set_text_allow_overlap(&constant(true));
    });
    test.run(Some("Overlap"), 0.01);
    assert_eq!(1, test.count(Some("pt0_overlap"), None, None));
}

// gtest: FeatureTracking.ZeroOpacityConstant
#[test]
fn zero_opacity_constant() {
    // Features appear when either icon or text opacity is non-zero (constant)
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);
    test.run(None, 0.0);
    assert_eq!(1, test.count(Some("pt0"), None, None));

    for (icon_opacity, text_opacity, expected) in [(0.0, 0.0, 0), (0.5, 0.0, 1), (0.0, 0.5, 1)] {
        test.with_symbol_layer(|layer| {
            layer.set_icon_opacity(&constant(icon_opacity));
            layer.set_text_opacity(&constant(text_opacity));
        });
        test.run(None, 0.0);
        assert_eq!(expected, test.count(Some("pt0"), None, None), "icon {icon_opacity}, text {text_opacity}");
    }
}

// gtest: FeatureTracking.ZeroOpacityExpression
#[test]
fn zero_opacity_expression() {
    // Features appear when either icon or text opacity is non-zero (zoom expression)
    let mut test = FeatureTrackingTest::with_geojson(r#"{"type": "FeatureCollection", "features": []}"#, camera(10.0));
    test.show(SYMBOL_LAYER_NAME);

    let expression = || {
        PropertyValue::Expression(PropertyExpression::from_box(
            interpolate(
                linear(),
                zoom(),
                vec![
                    (9.0, literal(ExpressionValue::Number(1.0))),
                    (10.0, literal(ExpressionValue::Number(0.0))),
                    (11.0, literal(ExpressionValue::Number(1.0))),
                ],
            ),
            None,
        ))
    };
    test.with_symbol_layer(|layer| {
        layer.set_icon_opacity(&expression());
        layer.set_text_opacity(&expression());
    });
    test.run(None, 0.0);
    assert_eq!(0, test.count(None, None, None));
}

// gtest: FeatureTracking.ZeroOpacityDataDriven
#[test]
fn zero_opacity_data_driven() {
    // Symbols appear when either icon or text opacity is non-zero (data-driven)
    let mut test = FeatureTrackingTest::new(camera(10.0));
    test.show(SYMBOL_LAYER_NAME);

    let expression = || PropertyValue::Expression(PropertyExpression::from_box(get(literal_str("opacity")), None));
    test.with_symbol_layer(|layer| {
        layer.set_icon_opacity(&expression());
        layer.set_text_opacity(&expression());
    });
    test.run(None, 0.0);
    assert_eq!(1, test.count(Some("pt0"), None, None));
    assert_eq!(0, test.count(Some("pt1"), None, None));
}

#[test]
fn ndc_bound_fill_extrusion() {
    let mut test = FeatureTrackingTest::new(CameraOptions { pitch: Some(45.0), ..camera(10.0) });
    let mut layer = FillExtrusionLayer::new("extrusion", SOURCE_NAME);
    layer.set_fill_extrusion_color(&constant(Color::new(0.0, 0.0, 1.0, 1.0)));
    layer.set_fill_extrusion_height(&constant(1000.0));
    test.map.style_mut().add_layer(Box::new(layer), None).expect("add layer");
    test.run(None, 0.0);

    assert_eq!(1, test.count(Some("poly0"), Some("extrusion"), Some(SOURCE_NAME)));
    test.check_features(Some("poly0"), Some("extrusion"), None, |info| {
        assert!(info.ndc_bound.min_x < info.ndc_bound.max_x, "{info:?}");
        assert!(info.ndc_bound.min_y < info.ndc_bound.max_y, "{info:?}");
        assert!(info.ndc_bound.min_x < 0.0 && info.ndc_bound.max_x > 0.0, "{info:?}");
        // The polygon spans the four tiles around the center.
        assert_eq!(4, info.tile_ids.len(), "{info:?}");
    });
}

/// `GlobeFeatureTrackingTest::geometry()`: `near` faces the camera; `far` is
/// inside the Mercator viewport and behind the globe's horizon.
const GLOBE_GEOMETRY: &str = r#"{"type": "FeatureCollection", "features": [
    {"type": "Feature", "id": "near", "properties": {"name": "0"},
     "geometry": {"type": "Point", "coordinates": [60, 50]}},
    {"type": "Feature", "id": "nearLine", "properties": {"name": "1"},
     "geometry": {"type": "LineString", "coordinates": [[55, 45], [65, 55]]}},
    {"type": "Feature", "id": "nearPolygon", "properties": {"name": "2"},
     "geometry": {"type": "Polygon", "coordinates": [[[55, 45], [65, 45], [65, 55], [55, 55], [55, 45]]]}},
    {"type": "Feature", "id": "far", "properties": {"name": "3"},
     "geometry": {"type": "Point", "coordinates": [85, 0]}},
    {"type": "Feature", "id": "farLine", "properties": {"name": "4"},
     "geometry": {"type": "LineString", "coordinates": [[83, -3], [88, 3]]}},
    {"type": "Feature", "id": "farPolygon", "properties": {"name": "5"},
     "geometry": {"type": "Polygon", "coordinates": [[[83, -3], [88, -3], [88, 3], [83, 3], [83, -3]]]}}
]}"#;

/// `GlobeFeatureTrackingTest`: the globe at zoom 0, centered on 0, 0.
fn globe_test() -> FeatureTrackingTest {
    let mut test = FeatureTrackingTest::with_geojson(
        GLOBE_GEOMETRY,
        CameraOptions { center: Some(LatLng::new(0.0, 0.0).expect("LatLng")), ..camera(0.0) },
    );
    set_projection(&mut test, ProjectionDefinition::new(ProjectionType::Globe));
    test.with_circle_layer(|layer| layer.set_circle_radius(&constant(4.0)));
    test
}

fn set_projection(test: &mut FeatureTrackingTest, definition: ProjectionDefinition) {
    let mut projection = Projection::new();
    projection.set_type(constant(definition));
    test.map.style_mut().set_projection(projection);
}

/// A filter that keeps the features whose `name` is `name`.
fn name_filter(name: &str) -> Filter {
    Filter::new(Some(Arc::from(eq(get(literal_str("name")), literal_str(name)))), None)
}

impl FeatureTrackingTest {
    /// Where `lat_lng` is, in NDC.
    fn ndc(&self, lat_lng: &LatLng) -> (f64, f64) {
        let pixel = self.map.pixel_for_lat_lng(lat_lng);
        let size = self.frontend.get_size();
        (pixel.x / f64::from(size.width) * 2.0 - 1.0, 1.0 - pixel.y / f64::from(size.height) * 2.0)
    }

    fn bound(&self, feature_id: &str, layer_id: &str) -> Option<NDCBound> {
        let mut result = None;
        self.map.get_rendered_features(Some(feature_id), Some(layer_id), None, |_, info| {
            result = Some(info.ndc_bound);
            true
        });
        result
    }

    /// Checks that the bound of `feature_id` in `layer_id` holds `lat_lng`,
    /// within `slack` of it on each side.
    fn expect_around(&self, feature_id: &str, layer_id: &str, lat_lng: &LatLng, slack: f64) {
        let bound = self.bound(feature_id, layer_id).unwrap_or_else(|| panic!("{feature_id} in {layer_id}"));
        let (x, y) = self.ndc(lat_lng);
        let context = format!("{feature_id} in {layer_id}: {bound:?} around ({x}, {y})");
        assert!(bound.min_x <= x && x <= bound.max_x, "{context}");
        assert!(bound.min_y <= y && y <= bound.max_y, "{context}");
        assert!(bound.min_x > x - slack && bound.max_x < x + slack, "{context}");
        assert!(bound.min_y > y - slack && bound.max_y < y + slack, "{context}");
    }

    /// Checks that the capture is the box of the green pixels the feature was
    /// drawn with, to within a pixel on each side.
    fn expect_captured_as_drawn(&self, feature_id: &str, layer_id: &str, image: &PremultipliedImage) {
        let rendered = green_bound(image).unwrap_or_else(|| panic!("{feature_id} in {layer_id}: nothing green"));
        let captured = self.bound(feature_id, layer_id).unwrap_or_else(|| panic!("{feature_id} in {layer_id}"));
        let pixel = 2.0 / f64::from(image.size.width);
        let context = format!("{feature_id} in {layer_id}: captured {captured:?}, rendered {rendered:?}");
        assert!((rendered.min_x - captured.min_x).abs() <= pixel, "{context}");
        assert!((rendered.max_x - captured.max_x).abs() <= pixel, "{context}");
        assert!((rendered.min_y - captured.min_y).abs() <= pixel, "{context}");
        assert!((rendered.max_y - captured.max_y).abs() <= pixel, "{context}");
    }
}

/// The NDC box of the green pixels of a rendered image.
fn green_bound(image: &PremultipliedImage) -> Option<NDCBound> {
    let (width, height) = (image.size.width as usize, image.size.height as usize);
    let mut result: Option<NDCBound> = None;
    for (i, pixel) in image.data.as_chunks::<4>().0.iter().enumerate() {
        let [r, g, b] = [i32::from(pixel[0]), i32::from(pixel[1]), i32::from(pixel[2])];
        if g > 100 && g > r + 50 && g > b + 50 {
            let (x, y) = (i % width, i / width);
            let ndc_x = (x as f64 + 0.5) / width as f64 * 2.0 - 1.0;
            let ndc_y = 1.0 - (y as f64 + 0.5) / height as f64 * 2.0;
            let bound = result.get_or_insert(NDCBound { min_x: ndc_x, max_x: ndc_x, min_y: ndc_y, max_y: ndc_y });
            bound.min_x = bound.min_x.min(ndc_x);
            bound.max_x = bound.max_x.max(ndc_x);
            bound.min_y = bound.min_y.min(ndc_y);
            bound.max_y = bound.max_y.max(ndc_y);
        }
    }
    result
}

fn near() -> LatLng {
    LatLng::new(50.0, 60.0).expect("LatLng")
}

// On the globe a feature is where the sphere puts it, which at this zoom is
// far from its Mercator position.
// gtest: FeatureTracking.GlobeNDCBound
#[test]
fn globe_ndc_bound() {
    let mut test = globe_test();
    for layer in [FILL_LAYER_NAME, LINE_LAYER_NAME, CIRCLE_LAYER_NAME, SYMBOL_LAYER_NAME] {
        test.show(layer);
    }
    test.run(None, 0.0);

    test.expect_around("nearPolygon", FILL_LAYER_NAME, &near(), 0.15);
    test.expect_around("nearLine", LINE_LAYER_NAME, &near(), 0.15);
    test.expect_around("near", CIRCLE_LAYER_NAME, &near(), 0.1);
    test.expect_around("near", SYMBOL_LAYER_NAME, &near(), 0.5);
}

// gtest: FeatureTracking.GlobeNDCBoundCirclePitchAlignment
#[test]
fn globe_ndc_bound_circle_pitch_alignment() {
    let mut test = globe_test();
    test.show(CIRCLE_LAYER_NAME);
    test.with_circle_layer(|layer| layer.set_circle_pitch_alignment(&constant(AlignmentType::Map)));
    for scale in [CirclePitchScaleType::Map, CirclePitchScaleType::Viewport] {
        test.with_circle_layer(|layer| layer.set_circle_pitch_scale(&constant(scale)));
        test.run(None, 0.0);
        test.expect_around("near", CIRCLE_LAYER_NAME, &near(), 0.1);
        assert_eq!(0, test.count(Some("far"), Some(CIRCLE_LAYER_NAME), None), "{scale:?}");
    }
}

// An extrusion rises along the sphere's normal, away from the globe's center
// on screen.
// gtest: FeatureTracking.GlobeNDCBoundFillExtrusion
#[test]
fn globe_ndc_bound_fill_extrusion() {
    let mut test = globe_test();
    test.show(FILL_LAYER_NAME);
    let mut extrusion_layer = FillExtrusionLayer::new("extrusion", SOURCE_NAME);
    extrusion_layer.set_fill_extrusion_height(&constant(500_000.0));
    test.map.style_mut().add_layer(Box::new(extrusion_layer), None).expect("add layer");
    test.run(None, 0.0);

    let flat = test.bound("nearPolygon", FILL_LAYER_NAME).expect("flat");
    let extruded = test.bound("nearPolygon", "extrusion").expect("extruded");
    assert!((extruded.min_x - flat.min_x).abs() <= 1e-6, "{extruded:?} {flat:?}");
    assert!((extruded.min_y - flat.min_y).abs() <= 1e-6, "{extruded:?} {flat:?}");
    assert!(extruded.max_x > flat.max_x + 0.01, "{extruded:?} {flat:?}");
    assert!(extruded.max_y > flat.max_y + 0.01, "{extruded:?} {flat:?}");
}

// A roof that rises above the horizon is drawn at the limb, and is captured
// where it is drawn.
// gtest: FeatureTracking.GlobeFillExtrusionAboveHorizon
#[test]
fn globe_fill_extrusion_above_horizon() {
    let mut test = globe_test();
    let mut extrusion_layer = FillExtrusionLayer::new("extrusion", SOURCE_NAME);
    extrusion_layer.set_fill_extrusion_height(&constant(500_000.0));
    extrusion_layer.set_fill_extrusion_color(&constant(Color::new(0.0, 1.0, 0.0, 1.0)));
    extrusion_layer.set_filter(&name_filter("5"));
    test.map.style_mut().add_layer(Box::new(extrusion_layer), None).expect("add layer");
    let image = test.run(None, 0.0).image;
    test.expect_captured_as_drawn("farPolygon", "extrusion", &image);
}

// While the projection hands over, the shaders blend the sphere with
// Mercator and clip nothing in the first fifth.
// gtest: FeatureTracking.GlobeCaptureDuringTransition
#[test]
fn globe_capture_during_transition() {
    let mut test = globe_test();
    set_projection(&mut test, ProjectionDefinition::between_names("mercator", "vertical-perspective", 0.1));
    test.show(FILL_LAYER_NAME);
    test.with_layer(FILL_LAYER_NAME, |layer| {
        layer.downcast_mut::<FillLayer>().expect("FillLayer").set_fill_color(&constant(Color::new(0.0, 1.0, 0.0, 1.0)));
    });
    for (feature_id, name) in [("nearPolygon", "2"), ("farPolygon", "5")] {
        test.with_layer(FILL_LAYER_NAME, |layer| layer.set_filter(&name_filter(name)));
        let image = test.run(None, 0.0).image;
        test.expect_captured_as_drawn(feature_id, FILL_LAYER_NAME, &image);
    }
}

// Close up, the visible part of the planet is smaller than the sampling grid
// of a feature from a low zoom tile.
// gtest: FeatureTracking.GlobeCaptureCloseUp
#[test]
fn globe_capture_close_up() {
    let mut test = globe_test();
    let options = GeoJSONOptions { maxzoom: 0, ..GeoJSONOptions::default() };
    let source = GeoJSONSource::new("lowZoom", Arc::new(options));
    let large = r#"{"type": "Feature", "id": "large", "properties": {},
        "geometry": {"type": "Polygon", "coordinates": [[[-40, -40], [40, -40], [40, 40], [-40, 40], [-40, -40]]]}}"#;
    source.set_geojson(&geojson::parse(large).expect("GeoJSON")).expect("set GeoJSON");
    let mut fill_layer = FillLayer::new("lowZoomFill", "lowZoom");
    fill_layer.set_fill_color(&constant(Color::new(0.0, 1.0, 0.0, 1.0)));
    {
        let style = test.map.style_mut();
        style.add_source(Box::new(source)).expect("add source");
        style.add_layer(Box::new(fill_layer), None).expect("add layer");
    }
    test.map.jump_to(&CameraOptions { pitch: Some(60.0), ..camera(11.0) });
    let image = test.run(None, 0.0).image;
    test.expect_captured_as_drawn("large", "lowZoomFill", &image);
}

// What the horizon hides is not rendered, wherever Mercator would have put it.
// gtest: FeatureTracking.GlobeOccluded
#[test]
fn globe_occluded() {
    let mut test = globe_test();
    for layer in [FILL_LAYER_NAME, LINE_LAYER_NAME, CIRCLE_LAYER_NAME, SYMBOL_LAYER_NAME] {
        test.show(layer);
    }
    test.run(None, 0.0);

    assert_eq!(0, test.count(Some("farPolygon"), Some(FILL_LAYER_NAME), None));
    assert_eq!(0, test.count(Some("farLine"), Some(LINE_LAYER_NAME), None));
    assert_eq!(0, test.count(Some("far"), Some(CIRCLE_LAYER_NAME), None));
    assert_eq!(0, test.count(Some("far"), Some(SYMBOL_LAYER_NAME), None));
    assert_eq!(1, test.count(Some("nearPolygon"), Some(FILL_LAYER_NAME), None));
}
