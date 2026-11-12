// Ports test/style/expression/dependency.test.cpp (its unused
// FormatSectionOverride helper is a renderer type).

use crate::collator_expression::CollatorExpression;
use crate::dsl::*;
use crate::expression::{Dependency, Expression};
use crate::semiliteral::Semiliteral;

// gtest: ExpressionDependencies.Get
#[test]
fn get_() {
    assert_eq!(Dependency::FEATURE, number(get_str("property"), None).dependencies());
}

// gtest: ExpressionDependencies.Id
#[test]
fn id_() {
    assert_eq!(Dependency::FEATURE, id().dependencies());
}

// gtest: ExpressionDependencies.Zoom
#[test]
fn zoom_() {
    assert_eq!(Dependency::ZOOM, zoom().dependencies());
}

// gtest: ExpressionDependencies.Convert
#[test]
fn convert() {
    assert_eq!(Dependency::FEATURE, to_color(string(get_str("color"), None), None).dependencies());
    assert_eq!(Dependency::FEATURE, to_string(number(string(get_str("color"), None), None), None).dependencies());
    assert_eq!(Dependency::FEATURE, boolean(string(get_str("color"), None), None).dependencies());
    assert_eq!(Dependency::FEATURE, to_formatted(to_color(string(get_str("color"), None), None), None).dependencies());
    assert_eq!(Dependency::FEATURE, to_variable_anchor_offset(string(get_str("color"), None), None).dependencies());

    // See `coercion.cpp`, `extraDependency`
    assert_eq!(Dependency::FEATURE, to_image(string(get_str("color"), None), None).dependencies());

    // See `collator_expression.cpp`, `extraDependency`
    assert_eq!(
        Dependency::FEATURE,
        create_expression_from_str(r#"["resolved-locale",["collator",{"locale": "de"}]]"#).unwrap().dependencies()
    );
    assert_eq!(
        Dependency::FEATURE | Dependency::ZOOM,
        CollatorExpression::new(gt(literal(1.0.into()), zoom()), literal(true.into()), Some(literal_str("en-us")))
            .dependencies()
    );
}

// gtest: ExpressionDependencies.Compare
#[test]
fn compare() {
    let c = || string(get_str("color"), None);
    assert_eq!(Dependency::FEATURE, eq(c(), c()).dependencies());
    assert_eq!(Dependency::FEATURE, lt(c(), c()).dependencies());
    assert_eq!(Dependency::FEATURE, gt(c(), c()).dependencies());
    assert_eq!(Dependency::FEATURE, eq(c(), c()).dependencies());
}

// gtest: ExpressionDependencies.Interpolate
#[test]
fn interpolate_() {
    let l = || literal(0.0.into());
    let n = || number(id(), None);
    assert_eq!(Dependency::FEATURE, step(n(), l(), 0.0, l()).dependencies());
    assert_eq!(Dependency::FEATURE, step(l(), n(), 0.0, l()).dependencies());
    assert_eq!(Dependency::FEATURE, step(l(), l(), 0.0, n()).dependencies());

    // The C++ test builds these (all stops at 0.0, so later ones replace
    // earlier ones) without checking the result.
    let _ = interpolate(linear(), n(), vec![(0.0, l()), (0.0, l()), (0.0, l())]);
    let _ = interpolate(linear(), l(), vec![(0.0, n()), (0.0, l()), (0.0, l())]);
    let _ = interpolate(linear(), l(), vec![(0.0, l()), (0.0, n()), (0.0, l())]);
    let _ = interpolate(linear(), l(), vec![(0.0, l()), (0.0, l()), (0.0, n())]);
}

// gtest: ExpressionDependencies.Image
#[test]
fn image_() {
    assert_eq!(Dependency::IMAGE, image(literal_str("airport-11")).dependencies());
    assert_eq!(Dependency::IMAGE | Dependency::FEATURE, image(get(literal_str("image_name"))).dependencies());
}

// gtest: ExpressionDependencies.Distance
#[test]
fn distance() {
    let point_geo_source = r#"{
         "type": "Point",
         "coordinates":[24.938492774963375,60.16980226227959]}"#;
    let expr_str = format!(r#"["distance", {point_geo_source} ]"#);
    let expression = create_expression_from_str(&expr_str).expect("distance expression");
    assert_eq!(Dependency::FEATURE, expression.dependencies());
}

// gtest: ExpressionDependencies.CustomLayer
#[test]
#[ignore = "TODO(port): CustomLayer (style/layers/custom_layer*) and CustomLayerProperties"]
fn custom_layer() {
    // TODO(port): body: `CustomLayerProperties{makeMutable<CustomLayer::Impl>("", nullptr)}.getDependencies()`
    // is `Dependency::None` (an mln_style test once CustomLayer exists).
}

// gtest: ExpressionDependencies.Semiliteral
#[test]
fn semiliteral() {
    let expression = Semiliteral::new(vec![get_str("property"), zoom()]);
    assert_eq!(Dependency::FEATURE | Dependency::ZOOM, expression.dependencies());
}
