// Ports test/style/property_expression.test.cpp.

use feature::Value as MbglValue;
use geometry::{Geometry, Point};
use mln_expression::dsl::create_expression_from_str;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryCollection;
use mln_util::geometry::FeatureType;
use mln_vec_map::VecSet;

use super::util::{StubGeometryTileFeature, property_map};
use crate::expression::dsl::{get, get_str, image, interpolate, linear, literal, literal_str, number, step, zoom};
use crate::expression::{Dependency, EvaluationContext, Image, Value};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::PropertyEvaluator;

fn one_integer() -> StubGeometryTileFeature {
    StubGeometryTileFeature::new(property_map(&[("property", MbglValue::Uint(1))]))
}

fn one_double() -> StubGeometryTileFeature {
    StubGeometryTileFeature::new(property_map(&[("property", MbglValue::Double(1.0))]))
}

fn one_string() -> StubGeometryTileFeature {
    StubGeometryTileFeature::new(property_map(&[("property", MbglValue::String("1".into()))]))
}

fn one_image() -> StubGeometryTileFeature {
    StubGeometryTileFeature::new(property_map(&[("image_name", MbglValue::String("maki-11".into()))]))
}

fn empty_tile_feature() -> StubGeometryTileFeature {
    StubGeometryTileFeature::new(property_map(&[]))
}

fn evaluate(value: PropertyValue<f32>, zoom: f32) -> f32 {
    let parameters = PropertyEvaluationParameters::new(zoom);
    value.evaluate(&PropertyEvaluator::new(&parameters, 0.0))
}

fn num(v: f64) -> Box<dyn crate::expression::Expression> {
    literal(Value::Number(v))
}

// gtest: PropertyExpression.Constant
#[test]
fn constant() {
    assert_eq!(2.0_f32, evaluate(PropertyValue::Constant(2.0), 0.0));
    assert_eq!(3.8_f32, evaluate(PropertyValue::Constant(3.8), 0.0));
    assert_eq!(22.0_f32, evaluate(PropertyValue::Constant(22.0), 0.0));
    assert_eq!(2.0_f32, evaluate(PropertyValue::Constant(2.0), 4.0));
    assert_eq!(3.8_f32, evaluate(PropertyValue::Constant(3.8), 4.0));
    assert_eq!(22.0_f32, evaluate(PropertyValue::Constant(22.0), 4.0));
    assert_eq!(2.0_f32, evaluate(PropertyValue::Constant(2.0), 22.0));
    assert_eq!(3.8_f32, evaluate(PropertyValue::Constant(3.8), 22.0));
    assert_eq!(22.0_f32, evaluate(PropertyValue::Constant(22.0), 22.0));

    assert_eq!(Dependency::NONE, PropertyValue::Constant(2.0_f32).get_dependencies());
}

// gtest: PropertyExpression.Expression
#[test]
fn expression() {
    let expression = PropertyExpression::<f32>::from_box(
        interpolate(linear(), zoom(), vec![(0.0, num(0.0)), (1.0, num(1.0))]),
        None,
    );
    assert_eq!(0.0, evaluate(PropertyValue::Expression(expression.clone()), 0.0));
    assert_eq!(0.5, evaluate(PropertyValue::Expression(expression.clone()), 0.5));
    assert_eq!(Dependency::ZOOM, expression.get_dependencies());
}

// gtest: PropertyExpression.Defaults
#[test]
fn defaults() {
    let no_default = PropertyExpression::<f32>::from_box(number(get_str("property"), None), None);
    let with_default = PropertyExpression::<f32>::from_box(number(get_str("property"), None), Some(0.0));
    assert_eq!(1.0_f32, with_default.evaluate_feature(&one_integer(), 2.0));
    assert_eq!(1.0_f32, with_default.evaluate_feature(&one_double(), 2.0));
    assert_eq!(0.0_f32, with_default.evaluate_feature(&one_string(), 2.0));
    assert_eq!(2.0_f32, no_default.evaluate_feature(&one_string(), 2.0));

    assert_eq!(Dependency::FEATURE, no_default.get_dependencies());
}

/// interpolate(linear(), number(get("property")), 1.0, literal(v))
fn inner(v: f64) -> Box<dyn crate::expression::Expression> {
    interpolate(linear(), number(get_str("property"), None), vec![(1.0, num(v))])
}

// gtest: PropertyExpression.ZoomInterpolation
#[test]
fn zoom_interpolation() {
    assert_eq!(
        40.0_f32,
        PropertyExpression::<f32>::from_box(
            interpolate(linear(), zoom(), vec![(0.0, inner(24.0)), (1.5, inner(36.0)), (3.0, inner(48.0))]),
            Some(0.0),
        )
        .evaluate_zoom_feature(2.0, &one_integer(), -1.0),
        "Should interpolate between stops"
    );

    assert_eq!(
        33.0_f32,
        PropertyExpression::<f32>::from_box(
            interpolate(linear(), zoom(), vec![(5.0, inner(33.0)), (10.0, inner(66.0))]),
            Some(0.0),
        )
        .evaluate_zoom_feature(0.0, &one_integer(), -1.0),
        "Use first stop output for input values from -inf to first stop"
    );

    assert_eq!(
        66.0_f32,
        PropertyExpression::<f32>::from_box(
            interpolate(linear(), zoom(), vec![(0.0, inner(33.0)), (10.0, inner(66.0))]),
            Some(0.0),
        )
        .evaluate_zoom_feature(20.0, &one_integer(), -1.0),
        "Use last stop output for input values from last stop to +inf"
    );

    assert_eq!(
        66.0_f32,
        PropertyExpression::<f32>::from_box(
            interpolate(linear(), zoom(), vec![(0.0, inner(33.0)), (10.0, inner(66.0))]),
            Some(0.0),
        )
        .evaluate_zoom_feature(10.0, &one_integer(), -1.0),
        "Should interpolate TO the last stop."
    );

    assert_eq!(
        33.0_f32,
        PropertyExpression::<f32>::from_box(
            interpolate(linear(), zoom(), vec![(0.0, inner(33.0)), (10.0, inner(66.0))]),
            Some(0.0),
        )
        .evaluate_zoom_feature(0.0, &one_integer(), -1.0),
        "Should interpolate TO the first stop"
    );
}

fn assert_near(expected: f32, actual: f32, tolerance: f32) {
    assert!((expected - actual).abs() <= tolerance, "expected {expected} ± {tolerance}, got {actual}");
}

// gtest: PropertyExpression.Issue8460
#[test]
fn issue8460() {
    let fn1 = PropertyExpression::<f32>::from_box(
        interpolate(linear(), zoom(), vec![(15.0, inner(0.0)), (15.2, inner(600.0))]),
        Some(0.0),
    );

    assert_near(0.0, fn1.evaluate_zoom_feature(15.0, &one_integer(), -1.0), 0.00);
    assert_near(300.0, fn1.evaluate_zoom_feature(15.1, &one_integer(), -1.0), 0.01);
    assert_near(600.0, fn1.evaluate_zoom_feature(15.2, &one_integer(), -1.0), 0.00);
    assert_near(600.0, fn1.evaluate_zoom_feature(16.0, &one_integer(), -1.0), 0.00);

    let fn2 = PropertyExpression::<f32>::from_box(
        interpolate(linear(), zoom(), vec![(15.0, inner(0.0)), (15.2, inner(300.0)), (18.0, inner(600.0))]),
        Some(0.0),
    );

    assert_near(0.0, fn2.evaluate_zoom_feature(15.0, &one_integer(), -1.0), 0.00);
    assert_near(150.0, fn2.evaluate_zoom_feature(15.1, &one_integer(), -1.0), 0.01);
    assert_near(300.0, fn2.evaluate_zoom_feature(15.2, &one_integer(), -1.0), 0.00);
    assert_near(385.71, fn2.evaluate_zoom_feature(16.0, &one_integer(), -1.0), 0.01);
    assert_near(600.0, fn2.evaluate_zoom_feature(18.0, &one_integer(), -1.0), 0.00);
    assert_near(600.0, fn2.evaluate_zoom_feature(19.0, &one_integer(), -1.0), 0.00);
}

// TODO(port): TEST(PropertyExpression, FormatSectionOverride) needs
// FormatSectionOverride (renderer, M5).

// gtest: PropertyExpression.ImageExpression
#[test]
fn image_expression() {
    let empty_set = VecSet::new();
    let available_images: VecSet<String> =
        ["maki-11", "airport-11", "bicycle-15"].iter().map(|s| s.to_string()).collect();

    // evaluation test without available images
    {
        let prop_expr = PropertyExpression::<Image>::from_box(image(literal_str("airport-11")), None);
        let evaluated_image = prop_expr.evaluate_feature_images(&empty_tile_feature(), &empty_set, Image::default());
        assert!(!evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "airport-11");

        assert_eq!(Dependency::IMAGE, prop_expr.get_dependencies());

        let dd_prop_expr = PropertyExpression::<Image>::from_box(image(get(literal_str("image_name"))), None);
        let evaluated_image = dd_prop_expr.evaluate_feature_images(&one_image(), &empty_set, Image::default());
        assert!(!evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "maki-11");

        let evaluated_image = dd_prop_expr.evaluate_feature_images(&empty_tile_feature(), &empty_set, Image::default());
        assert!(!evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "");
    }
    // evaluation test with available images
    {
        let prop_expr = PropertyExpression::<Image>::from_box(image(literal_str("airport-11")), None);
        let evaluated_image =
            prop_expr.evaluate_feature_images(&empty_tile_feature(), &available_images, Image::default());
        assert!(evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "airport-11");
        assert_eq!(Dependency::IMAGE, prop_expr.get_dependencies());
        let dd_prop_expr = PropertyExpression::<Image>::from_box(image(get(literal_str("image_name"))), None);
        let evaluated_image = dd_prop_expr.evaluate_feature_images(&one_image(), &available_images, Image::default());
        assert!(evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "maki-11");
        assert_eq!(Dependency::IMAGE | Dependency::FEATURE, dd_prop_expr.get_dependencies());
        let evaluated_image =
            dd_prop_expr.evaluate_feature_images(&empty_tile_feature(), &available_images, Image::default());
        assert!(!evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "");
    }
    // evaluation with zoom
    {
        let expr = step(zoom(), image(literal_str("airport-11")), 18.0, image(literal_str("bicycle-15")));
        let prop_expr = PropertyExpression::<Image>::from_box(expr, None);
        let evaluated_image =
            prop_expr.evaluate_zoom_feature_images(0.0, &empty_tile_feature(), &available_images, Image::default());
        assert!(evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "airport-11");
        let evaluated_image =
            prop_expr.evaluate_zoom_feature_images(18.0, &empty_tile_feature(), &available_images, Image::default());
        assert!(evaluated_image.is_available());
        assert_eq!(evaluated_image.id(), "bicycle-15");
        assert_eq!(Dependency::IMAGE | Dependency::ZOOM, prop_expr.get_dependencies());
    }
}

/// `convertGeometry(geometry, canonical)` (lon/lat to tile coordinates).
fn convert_geometry(geometry: Geometry<f64>, canonical: &CanonicalTileID) -> GeometryCollection {
    mln_tile::geometry_tile_data::convert_geometry_to_tile(&geometry, canonical)
}

fn pt(x: f64, y: f64) -> Point<f64> {
    Point::new(x, y)
}

fn pts(points: &[(f64, f64)]) -> Vec<Point<f64>> {
    points.iter().map(|&(x, y)| pt(x, y)).collect()
}

/// `createExpression(R"(["<op>", <geojson> ])")`.
fn create_geo_expression(op: &str, geojson: &str) -> Option<Box<dyn crate::expression::Expression>> {
    create_expression_from_str(&format!(r#"["{op}", {geojson} ]"#))
}

// gtest: PropertyExpression.WithinExpression
#[test]
fn within_expression() {
    let canonical_tile_id = CanonicalTileID::new(3, 3, 3);

    // geoJSON source must be Polygon.(Currently only supports Polygon)
    let invalid_geo_source = r#"{
      "type": "LineString",
      "coordinates": [[0, 0], [0, 5], [5, 5], [5, 0]]
    }"#;
    let valid_geo_source = r#"
    {
      "type": "Polygon",
      "coordinates": [
        [
          [-11.689453125, -9.79567758282973],
          [2.021484375, -10.012129557908128],
          [-15.99609375, -17.392579271057766],
          [-5.9765625, -5.659718554577273],
          [-16.259765625, -3.7327083213358336],
          [-17.75390625, -12.897489183755892],
          [-17.138671875, -21.002471054356715],
          [4.482421875, -16.8886597873816],
          [3.076171875, -7.01366792756663],
          [ -5.9326171875, 0.6591651462894632],
          [-16.1279296875, 1.4939713066293239],
          [-11.689453125, -9.79567758282973]
        ]
      ]
        }"#;

    // evaluation test with invalid geojson source
    {
        let expression = create_geo_expression("within", invalid_geo_source);
        assert!(expression.is_none());
    }

    // evaluation test with valid geojson source
    let expression = create_geo_expression("within", valid_geo_source);
    assert!(expression.is_some());
    let prop_expr = PropertyExpression::<bool>::from_box(expression.unwrap(), None);

    assert_eq!(Dependency::FEATURE, prop_expr.get_dependencies());

    let eval = |feature: &StubGeometryTileFeature| {
        prop_expr.evaluate(
            &EvaluationContext::from_feature(Some(feature)).with_canonical_tile_id(Some(&canonical_tile_id)),
            false,
        )
    };

    // evaluation test with valid geojson source but FeatureType is not Point/LineString
    // (currently only support FeatureType::Point and FeatureType::LineString)
    {
        // testPoly is inside polygon, but will return false
        let test_ring = vec![pts(&[
            (-9.228515625, -17.560246503294888),
            (-2.4609375, -16.04581345375217),
            (-9.228515625, -17.560246503294888),
        ])];
        let geo_poly = convert_geometry(Geometry::Polygon(test_ring), &canonical_tile_id);
        let poly_feature = StubGeometryTileFeature::with_geometry(FeatureType::Polygon, geo_poly);

        assert!(!eval(&poly_feature));
    }
    // evaluation test with valid geojson source and valid linestring features
    {
        // testLine is inside polygon
        let test_line0 = pts(&[(-9.228515625, -17.560246503294888), (-2.4609375, -16.04581345375217)]);
        let geo_line0 = convert_geometry(Geometry::LineString(test_line0), &canonical_tile_id);
        let line_feature0 = StubGeometryTileFeature::with_geometry(FeatureType::LineString, geo_line0);

        // testLine is intersecting polygon even though end points are all inside polygon, will return false
        let test_line1 = pts(&[(-10.4150390625, -10.082445532162465), (-8.8275146484375, -9.194292714912638)]);
        let geo_line1 = convert_geometry(Geometry::LineString(test_line1), &canonical_tile_id);
        let line_feature1 = StubGeometryTileFeature::with_geometry(FeatureType::LineString, geo_line1);

        assert!(eval(&line_feature0));
        assert!(!eval(&line_feature1));
    }

    // evaluation test with valid geojson source and valid point features
    {
        let get_point_feature = |x: f64, y: f64| {
            let geo_point = convert_geometry(Geometry::Point(pt(x, y)), &canonical_tile_id);
            StubGeometryTileFeature::with_geometry(FeatureType::Point, geo_point)
        };

        // check `within` algorithm
        assert!(!eval(&get_point_feature(-10.72265625, -7.27529233637217)));
        assert!(!eval(&get_point_feature(-7.646484374999999, -12.382928338487396)));
        assert!(!eval(&get_point_feature(-15.2490234375, -2.986927393334863)));
        assert!(!eval(&get_point_feature(-10.590820312499998, 2.4601811810210052)));
        assert!(eval(&get_point_feature(-3.9990234375, -4.915832801313164)));
        assert!(eval(&get_point_feature(-1.1865234375, -16.63619187839765)));
        assert!(eval(&get_point_feature(-15.5126953125, -11.73830237143684)));

        // On the boundary
        assert!(!eval(&get_point_feature(3.076171875, -7.01366792756663)));
        assert!(!eval(&get_point_feature(-11.689453125, -9.79567758282973)));
        assert!(!eval(&get_point_feature(2.021484375, -10.012129557908128)));
        assert!(!eval(&get_point_feature(-15.99609375, -17.392579271057766)));
        assert!(!eval(&get_point_feature(-5.9765625, -5.659718554577273)));
        assert!(!eval(&get_point_feature(-16.259765625, -3.7327083213358336)));
        assert!(!eval(&get_point_feature(-17.75390625, -12.897489183755892)));
        assert!(!eval(&get_point_feature(-17.138671875, -21.002471054356715)));
        assert!(!eval(&get_point_feature(4.482421875, -16.8886597873816)));
        assert!(!eval(&get_point_feature(3.076171875, -7.01366792756663)));
        assert!(!eval(&get_point_feature(-5.9326171875, 0.6591651462894632)));
        assert!(!eval(&get_point_feature(-16.1279296875, 1.4939713066293239)));
        assert!(!eval(&get_point_feature(-11.689453125, -9.79567758282973)));
    }
}

// gtest: PropertyExpression.WithinExpressionAntiMeridian
#[test]
fn within_expression_anti_meridian() {
    let canonical_tile_id = CanonicalTileID::new(0, 0, 0);
    let eval = |prop_expr: &PropertyExpression<bool>, feature: &StubGeometryTileFeature| {
        prop_expr.evaluate(
            &EvaluationContext::from_feature(Some(feature)).with_canonical_tile_id(Some(&canonical_tile_id)),
            false,
        )
    };

    // evaluation test with line geometries
    {
        let polygon1 = r#"
       {
         "type": "Polygon",
         "coordinates": [[[-190, 0], [-178, 0], [-178, 10], [-190, 10], [-190, 0]]]
       }"#;
        // evaluation test with valid geojson source
        let expression = create_geo_expression("within", polygon1);
        assert!(expression.is_some());
        let prop_expr = PropertyExpression::<bool>::from_box(expression.unwrap(), None);

        let geo_line0 =
            convert_geometry(Geometry::LineString(pts(&[(-183.0, 5.0), (-179.0, 1.0)])), &canonical_tile_id);
        let line_feature0 = StubGeometryTileFeature::with_geometry(FeatureType::LineString, geo_line0);

        let geo_line1 = convert_geometry(Geometry::LineString(pts(&[(183.0, 5.0), (181.0, 1.0)])), &canonical_tile_id);
        let line_feature1 = StubGeometryTileFeature::with_geometry(FeatureType::LineString, geo_line1);

        assert!(eval(&prop_expr, &line_feature0));
        assert!(!eval(&prop_expr, &line_feature1));
    }

    // evaluation test with point geometries
    {
        let polygon2 = r#"
       {
         "type": "Polygon",
         "coordinates":  [[[-185.0, 60.0], [-175.0, 60.0], [-175.0, 65.0], [-185.0, 65.0], [-185.0, 60.0]]]
       }"#;
        // evaluation test with valid geojson source
        let expression = create_geo_expression("within", polygon2);
        assert!(expression.is_some());
        let prop_expr = PropertyExpression::<bool>::from_box(expression.unwrap(), None);

        let get_point_feature = |x: f64, y: f64| {
            let geo_point = convert_geometry(Geometry::Point(pt(x, y)), &canonical_tile_id);
            StubGeometryTileFeature::with_geometry(FeatureType::Point, geo_point)
        };

        // check `within` algorithm
        assert!(eval(&prop_expr, &get_point_feature(177.0, 62.5)));
        assert!(eval(&prop_expr, &get_point_feature(180.0, 62.5)));
        assert!(eval(&prop_expr, &get_point_feature(-180.0, 62.5)));
        assert!(!eval(&prop_expr, &get_point_feature(-190.0, 62.5)));
    }
}

/// `EXPECT_NEAR` on doubles.
fn expect_near(expected: f64, actual: f64, tolerance: f64) {
    assert!((expected - actual).abs() <= tolerance, "expected {expected} ± {tolerance}, got {actual}");
}

/// `mln::util::read_file("test/fixtures/geometry_data/<name>")`.
fn read_geometry_data(name: &str) -> String {
    super::util::read_file(&format!("test/fixtures/geometry_data/{name}"))
}

// gtest: PropertyExpression.DistanceExpression
#[test]
#[ignore = "TODO(port): the \"distance\" expression (src/mln/style/expression/distance.cpp) fails to parse"]
fn distance_expression() {
    let invalid_result = f64::NAN;
    let canonical_tile_id = CanonicalTileID::new(15, 18653, 9484);
    let eval = |prop_expr: &PropertyExpression<f64>, feature: &StubGeometryTileFeature| {
        prop_expr.evaluate(
            &EvaluationContext::from_feature(Some(feature)).with_canonical_tile_id(Some(&canonical_tile_id)),
            invalid_result,
        )
    };
    let distance_expr = |geojson: &str| {
        let expression = create_geo_expression("distance", geojson);
        assert!(expression.is_some(), "{geojson}");
        PropertyExpression::<f64>::from_box(expression.unwrap(), None)
    };
    let feature = |type_: FeatureType, geometry: Geometry<f64>| {
        StubGeometryTileFeature::with_geometry(type_, convert_geometry(geometry, &canonical_tile_id))
    };
    let get_feature = |name: &str, type_: FeatureType| {
        let geometry_input = read_geometry_data(name);
        let geometry = match geojson::parse(&geometry_input).expect("fixture GeoJSON") {
            geojson::GeoJSON::Geometry(geometry_set) => geometry_set,
            geojson::GeoJSON::Feature(feature) => feature.geometry,
            geojson::GeoJSON::FeatureCollection(features) => features[0].geometry.clone(),
        };
        feature(type_, geometry)
    };

    // Parsing error with invalid geojson source
    {
        // geoJSON source must be Point or LineString.
        let invalid_geo_source = r#"{
            "type": "Unknown",
            "coordinates": [[[0, 0], [0, 5], [5, 5], [5, 0], [0, 0]]]
            }"#;
        assert!(create_geo_expression("distance", invalid_geo_source).is_none());
    }

    // Parsing error with extra arguments
    {
        let invalid_geo_source = r#"{
            "type": "Point",
            "coordinates": [0, 0],
            }"#;
        let expression = create_expression_from_str(&format!(r#"["distance", {invalid_geo_source}, "ExtraArgument"]"#));
        assert!(expression.is_none());
    }

    let point_geo_source = r#"{
             "type": "Point",
             "coordinates":
               [
                   24.938492774963375,
                   60.16980226227959
               ]
              }"#;

    // Evaluation test with Point to Point distance by using different units
    {
        let point_feature = feature(FeatureType::Point, Geometry::Point(pt(24.931159615516663, 60.16733387258923)));

        // Default Unit: meters
        let prop_expr = distance_expr(point_geo_source);
        assert_eq!(Dependency::FEATURE, prop_expr.get_dependencies());

        expect_near(491.307, eval(&prop_expr, &point_feature), 0.01);
    }

    // Evaluation test with Point to MultiPoint distance
    {
        let prop_expr = distance_expr(point_geo_source);
        let multi_point = feature(
            FeatureType::Point,
            Geometry::MultiPoint(pts(&[
                (24.931159615516663, 60.16733387258923),
                (24.936089515686035, 60.169033745694826),
            ])),
        );
        expect_near(158.541, eval(&prop_expr, &multi_point), 0.01);
    }

    // Evaluation test with Point to LineString distance
    {
        let prop_expr = distance_expr(point_geo_source);
        let line_feature = feature(
            FeatureType::LineString,
            Geometry::LineString(pts(&[
                (24.93621826171875, 60.17131789507754),
                (24.941453933715817, 60.170229208170014),
                (24.941110610961914, 60.16850004304534),
            ])),
        );
        expect_near(107.211, eval(&prop_expr, &line_feature), 0.01);
    }

    // Evaluation test with Point to MultiLineString distance
    {
        let prop_expr = distance_expr(point_geo_source);
        let line_feature = feature(
            FeatureType::LineString,
            Geometry::MultiLineString(vec![
                pts(&[
                    (24.93621826171875, 60.17131789507754),
                    (24.941453933715817, 60.170229208170014),
                    (24.941110610961914, 60.16850004304534),
                ]),
                pts(&[(24.937773942947388, 60.16940199546824), (24.933900833129883, 60.17007977773949)]),
            ]),
        );
        expect_near(59.845, eval(&prop_expr, &line_feature), 0.01);
    }

    // Evaluation test with MultiPoint to MultiPoint distance
    {
        let multi_points1 = read_geometry_data("multi_point_1.geojson");
        let multi_points_feature = get_feature("multi_point_2.geojson", FeatureType::Point);
        let prop_expr = distance_expr(&multi_points1);
        expect_near(283.60, eval(&prop_expr, &multi_points_feature), 0.1);
    }

    // Evaluation test with MultiPoint to LineString distance
    {
        let multi_points1 = read_geometry_data("multi_point_1.geojson");
        let multi_line_feature = get_feature("line_string_2.geojson", FeatureType::LineString);
        let prop_expr = distance_expr(&multi_points1);
        expect_near(258.691, eval(&prop_expr, &multi_line_feature), 0.01);

        // ----------------- switch feature and argument -----------------
        let multi_line2 = read_geometry_data("line_string_2.geojson");
        let multi_points_feature = get_feature("multi_point_1.geojson", FeatureType::Point);
        let prop_expr1 = distance_expr(&multi_line2);
        expect_near(258.691, eval(&prop_expr1, &multi_points_feature), 0.01);
    }

    // Evaluation test with LineString to LineString distance
    {
        let line_string1 = read_geometry_data("line_string_1.geojson");
        let line_feature = get_feature("line_string_2.geojson", FeatureType::LineString);
        let prop_expr = distance_expr(&line_string1);
        expect_near(180.408, eval(&prop_expr, &line_feature), 0.01);
    }

    // Evaluation test with MultiPoints to MutltiLineString distance
    {
        let multi_points = read_geometry_data("multi_point_2.geojson");
        let multi_line_feature = get_feature("multi_line_string_1.geojson", FeatureType::LineString);
        let prop_expr = distance_expr(&multi_points);
        expect_near(937.632, eval(&prop_expr, &multi_line_feature), 0.01);
    }

    // Evaluation test with LineString to MutltiLineString distance
    {
        let line_string1 = read_geometry_data("line_string_1.geojson");
        let multi_line_feature = get_feature("multi_line_string_2.geojson", FeatureType::LineString);
        let prop_expr = distance_expr(&line_string1);
        expect_near(17.511, eval(&prop_expr, &multi_line_feature), 0.01);
    }

    // Evaluation test with MultiLineString to MutltiLineString distance
    {
        let multi_line_string1 = read_geometry_data("multi_line_string_1.geojson");
        let multi_line_feature = get_feature("multi_line_string_2.geojson", FeatureType::LineString);
        let prop_expr = distance_expr(&multi_line_string1);
        expect_near(384.109, eval(&prop_expr, &multi_line_feature), 0.01);
    }

    // Evaluation test with MultiPoint to MultiPolygon distance
    {
        let multi_polygon = read_geometry_data("multi_polygon_1.geojson");
        let prop_expr = distance_expr(&multi_polygon);

        let multi_point_feature1 = get_feature("multi_point_3.geojson", FeatureType::Point);
        expect_near(27.917, eval(&prop_expr, &multi_point_feature1), 0.01);

        let multi_point_feature2 = get_feature("multi_point_2.geojson", FeatureType::Point);
        expect_near(0.0, eval(&prop_expr, &multi_point_feature2), 0.01);
    }

    // Evaluation test with MultiLine to MultiPolygon distance
    {
        let multi_polygon = read_geometry_data("multi_polygon_1.geojson");
        let prop_expr = distance_expr(&multi_polygon);

        let multi_line_feature1 = get_feature("multi_line_string_1.geojson", FeatureType::LineString);
        expect_near(193.450, eval(&prop_expr, &multi_line_feature1), 0.01);

        let multi_line_feature2 = get_feature("multi_line_string_2.geojson", FeatureType::LineString);
        expect_near(0.0, eval(&prop_expr, &multi_line_feature2), 0.01);
    }

    // Evaluation test with Polygon to MultiPolygon distance
    {
        let multi_polygon = read_geometry_data("multi_polygon_1.geojson");
        let prop_expr = distance_expr(&multi_polygon);

        let polygon_feature = feature(
            FeatureType::Polygon,
            Geometry::Polygon(vec![pts(&[
                (24.94171142578125, 60.16777419352904),
                (24.948577880859375, 60.16777419352904),
                (24.948577880859375, 60.170122472217564),
                (24.94171142578125, 60.170122472217564),
                (24.94171142578125, 60.16777419352904),
            ])]),
        );
        expect_near(203.876, eval(&prop_expr, &polygon_feature), 0.01);
    }

    // Evaluation test with MultiPolygon to MultiPolygon distance
    {
        let multi_polygon = read_geometry_data("multi_polygon_1.geojson");
        let prop_expr = distance_expr(&multi_polygon);

        let multi_polygon_feature = get_feature("multi_polygon_2.geojson", FeatureType::Polygon);
        expect_near(21.446, eval(&prop_expr, &multi_polygon_feature), 0.01);
    }

    // Evaluation test with LineString/Polygon to Polygon distance with special cases
    {
        let multi_polygon = r#"{
                    "type": "Polygon",
                          "coordinates": [[[24.937891960144043, 60.16730451765011],
                                           [24.94892120361328, 60.16730451765011],
                                           [24.94892120361328, 60.17010112498546],
                                           [24.937891960144043, 60.17010112498546],
                                           [24.937891960144043, 60.16730451765011]]]}"#;
        let prop_expr = distance_expr(multi_polygon);

        // polygon1 is fully inside polygon
        let polygon_feature1 = feature(
            FeatureType::Polygon,
            Geometry::Polygon(vec![pts(&[
                (24.941797256469727, 60.16850004304534),
                (24.94235515594482, 60.16850004304534),
                (24.94235515594482, 60.169439353910285),
                (24.941797256469727, 60.169439353910285),
                (24.941797256469727, 60.16850004304534),
            ])]),
        );
        expect_near(0.0, eval(&prop_expr, &polygon_feature1), 0.01);

        // polygon2's line is intersecting polygon, but no point is inside polygon
        let polygon_feature2 = feature(
            FeatureType::Polygon,
            Geometry::Polygon(vec![pts(&[
                (24.943385124206543, 60.166066249059554),
                (24.94596004486084, 60.166066249059554),
                (24.94596004486084, 60.1714246271462),
                (24.943385124206543, 60.1714246271462),
                (24.943385124206543, 60.166066249059554),
            ])]),
        );
        expect_near(0.0, eval(&prop_expr, &polygon_feature2), 0.01);

        // one of polygon3's point is inside polygon
        let polygon_feature3 = feature(
            FeatureType::Polygon,
            Geometry::Polygon(vec![pts(&[
                (24.94797706604004, 60.1694607015724),
                (24.952354431152344, 60.1694607015724),
                (24.952354431152344, 60.17082692309542),
                (24.94797706604004, 60.17082692309542),
                (24.94797706604004, 60.1694607015724),
            ])]),
        );
        expect_near(0.0, eval(&prop_expr, &polygon_feature3), 0.01);

        // lineString1 is within polygon
        let line_feature1 = feature(
            FeatureType::LineString,
            Geometry::LineString(pts(&[
                (24.947097301483154, 60.16921520262075),
                (24.947311878204346, 60.1680197032483),
            ])),
        );
        expect_near(0.0, eval(&prop_expr, &line_feature1), 0.01);

        // lineString2 is intersecting the polygon but its points are outside the polygon
        let line_feature2 = feature(
            FeatureType::LineString,
            Geometry::LineString(pts(&[
                (24.93999481201172, 60.17116846959888),
                (24.940037727355957, 60.16542574699484),
            ])),
        );
        expect_near(0.0, eval(&prop_expr, &line_feature2), 0.01);
    }
}
