// The global state tests on style types (PropertyExpression, PropertyValue,
// Filter, ColorRampPropertyValue). The expression-only ones are in
// mln_expression's tests, the FormatSectionOverride one in
// tests/format_section_override_test.rs.

use std::sync::Arc;

use feature::Value;
use mln_conversion::Convertible;
use mln_util::Color;
use mln_vec_map::VecSet;

use super::util::parse_json;
use crate::color_ramp_property_value::ColorRampPropertyValue;
use crate::conversion::{Converter, Error, convert_property_value};
use crate::expression::{Dependency, EvaluationContext, Expression, GlobalStateMap, ParsingContext, Type};
use crate::filter::Filter;
use crate::property_expression::PropertyExpression;

fn parse_expression(json: &str, expected: Option<Type>) -> Option<Box<dyn Expression>> {
    let document = parse_json(json);
    let mut ctx = match expected {
        Some(t) => ParsingContext::with_expected(t),
        None => ParsingContext::new(),
    };
    ctx.parse_expression(&Convertible::from(&document), None)
}

fn state(entries: &[(&str, Value)]) -> GlobalStateMap {
    entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

// gtest: GlobalStateExpression.PropertyExpressionCapturesGlobalState
#[test]
fn property_expression_captures_global_state() {
    let expr = parse_expression(r#"["to-number", ["global-state", "width"]]"#, Some(Type::Number)).unwrap();

    let mut property_expression = PropertyExpression::<f32>::from_box(expr, None);
    assert!(property_expression.is_feature_constant());
    assert!(property_expression.is_zoom_constant());
    assert!(!property_expression.is_runtime_constant());

    // Without any state, the expression falls back to the default.
    assert_eq!(0.0_f32, property_expression.evaluate(&EvaluationContext::new(0.0), 0.0));

    let state = Arc::new(state(&[("width", Value::Double(3.0))]));

    // State provided via the evaluation context.
    assert_eq!(
        3.0_f32,
        property_expression.evaluate(&EvaluationContext::new(0.0).with_global_state(Some(state.as_ref())), 0.0)
    );

    // Captured state is used when the context has none.
    property_expression.capture_global_state(Some(Arc::clone(&state)));
    assert_eq!(3.0_f32, property_expression.evaluate(&EvaluationContext::new(0.0), 0.0));

    // An explicitly provided state takes precedence over the captured one.
    let other = self::state(&[("width", Value::Double(7.0))]);
    assert_eq!(
        7.0_f32,
        property_expression.evaluate(&EvaluationContext::new(0.0).with_global_state(Some(&other)), 0.0)
    );
}

// gtest: GlobalStateExpression.PropertyValueConversionKeepsExpression
#[test]
fn property_value_conversion_keeps_expression() {
    let json = parse_json(r#"["to-number", ["global-state", "width"]]"#);

    let mut error = Error::default();
    let converted = convert_property_value::<f32>(&Convertible::from(&json), &mut error, true, false);
    let converted = converted.unwrap_or_else(|| panic!("{}", error.message));
    // The property value must keep the expression rather than folding it into
    // a constant, so that changes to the global state can be picked up.
    assert!(converted.is_expression());
    assert!(converted.get_dependencies().intersects(Dependency::GLOBAL_STATE));
}

// gtest: GlobalStateExpression.Filter
#[test]
fn filter() {
    let json = parse_json(r#"["to-boolean", ["global-state", "showFeatures"]]"#);

    let mut error = Error::default();
    let filter = <Filter as Converter>::convert(&Convertible::from(&json), &mut error);
    let filter = filter.unwrap_or_else(|| panic!("{}", error.message));

    let shown = state(&[("showFeatures", Value::Bool(true))]);
    let hidden = state(&[("showFeatures", Value::Bool(false))]);

    assert!(filter.evaluate(&EvaluationContext::default().with_global_state(Some(&shown))));
    assert!(!filter.evaluate(&EvaluationContext::default().with_global_state(Some(&hidden))));
    assert!(!filter.evaluate(&EvaluationContext::default()));
}

// gtest: GlobalStateExpression.ColorRamp
#[test]
fn color_ramp() {
    let json = parse_json(r#"["to-color", ["global-state", "hotColor"]]"#);

    let mut ctx = ParsingContext::with_expected(Type::Color);
    let parsed = ctx.parse_expression(&Convertible::from(&json), None).expect("parsed");

    let ramp = ColorRampPropertyValue::new(Some(Arc::from(parsed)));
    assert!(ramp.get_dependencies().intersects(Dependency::GLOBAL_STATE));

    let state = state(&[("hotColor", Value::String("red".to_owned()))]);
    assert_eq!(Color::parse("red").unwrap(), ramp.evaluate_ramp(0.5, Some(&state)));

    // Without global state the expression fails to produce a color; the
    // evaluation must degrade to a default color instead of crashing.
    assert_eq!(Color::default(), ramp.evaluate_ramp(0.5, None));
}

// gtest: GlobalStateExpression.PropertyExpressionGlobalStateRefs
#[test]
fn property_expression_global_state_refs() {
    let expr = parse_expression(r#"["to-number", ["global-state", "width"]]"#, Some(Type::Number)).unwrap();
    let property_expression = PropertyExpression::<f32>::from_box(expr, None);
    assert_eq!(Some(&VecSet::from(["width".to_owned()])), property_expression.get_global_state_refs());

    let plain = parse_expression(r#"["to-number", ["get", "width"]]"#, Some(Type::Number)).unwrap();
    let plain_expression = PropertyExpression::<f32>::from_box(plain, None);
    assert_eq!(None, plain_expression.get_global_state_refs());
}
