// Ports test/renderer/feature_tracking.test.cpp: the features a map with
// `MapOptions::with_rendered_feature_info` reports as rendered in the last
// frame, and their bounds in normalized device coordinates.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::{HeadlessFrontend, RenderResult};
use mln_map::{CameraOptions, FeatureInfo, Map, MapDebugOptions, MapMode, MapOptions, NullMapObserver};
use mln_storage::Kind;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_style::PropertyValue;
use mln_style::expression::Value as ExpressionValue;
use mln_style::expression::dsl::{format, get, interpolate, linear, literal, literal_str, zoom};
use mln_style::layer::Layer;
use mln_style::layers::background_layer::BackgroundLayer;
use mln_style::layers::circle_layer::CircleLayer;
use mln_style::layers::fill_extrusion_layer::FillExtrusionLayer;
use mln_style::layers::fill_layer::FillLayer;
use mln_style::layers::line_layer::LineLayer;
use mln_style::layers::symbol_layer::SymbolLayer;
use mln_style::property_expression::PropertyExpression;
use mln_style::sources::geojson_source::GeoJSONSource;
use mln_style::style_impl::LayerRefMut;
use mln_style_types::{AlignmentType, CirclePitchScaleType, SymbolAnchorType, SymbolPlacementType, VisibilityType};
use mln_util::Color;
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
        let frontend = HeadlessFrontend::with_pixel_ratio(1.0);
        let file_source = Arc::new(StubFileSource::new(ResponseType::Asynchronous));
        file_source.set_response(Kind::Glyphs, |_| Some(file_response(GLYPH_PATH)));
        let mut map = Map::with_file_source(
            &frontend,
            Rc::new(NullMapObserver),
            file_source,
            MapOptions::new()
                .with_map_mode(MapMode::Static)
                .with_size(frontend.get_size())
                .with_rendered_feature_info(true),
        )
        .expect("map");

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
            let mut style = map.get_style_mut();
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
    /// compares with test/fixtures/feature_tracking/<image>/expected.png.
    fn run(&mut self, image: Option<&str>, image_threshold: f64) -> RenderResult {
        let result = self.frontend.render(&mut self.map).expect("render");
        if let Some(image) = image {
            check_image(&format!("{FIXTURE_BASE}feature_tracking/{image}"), &result.image, image_threshold, 0.1);
        }
        result
    }

    fn with_layer(&mut self, id: &str, f: impl FnOnce(&mut LayerRefMut<'_>)) {
        let mut style = self.map.get_style_mut();
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
    CameraOptions::default().with_zoom(Some(zoom_level))
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
    let mut test = FeatureTrackingTest::new(camera(10.0).with_pitch(Some(60.0)));
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
    let pitched = test.map.get_camera_options(None).with_pitch(Some(60.0));
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
    let mut test = FeatureTrackingTest::new(camera(10.0).with_pitch(Some(45.0)));
    let mut layer = FillExtrusionLayer::new("extrusion", SOURCE_NAME);
    layer.set_fill_extrusion_color(&constant(Color::new(0.0, 0.0, 1.0, 1.0)));
    layer.set_fill_extrusion_height(&constant(1000.0));
    test.map.get_style_mut().add_layer(Box::new(layer), None).expect("add layer");
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
