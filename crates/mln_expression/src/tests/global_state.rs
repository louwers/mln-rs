use feature::Value as MbglValue;
use mln_conversion::Convertible;
use mln_vec_map::VecSet;

use crate::expression::{
    Dependency, EvaluationContext, Expression, GlobalStateMap, Kind, collect_global_state_refs,
    global_state_refs_intersect,
};
use crate::is_constant::{is_feature_constant, is_runtime_constant, is_zoom_constant};
use crate::parsing_context::ParsingContext;
use crate::type_::Type;
use crate::value::Value;

fn parse_expression(json: &str, expected: Option<Type>) -> Option<Box<dyn Expression>> {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut ctx = match expected {
        Some(t) => ParsingContext::with_expected(t),
        None => ParsingContext::new(),
    };
    ctx.parse_expression(&Convertible::from(&document), None)
}

fn state(entries: &[(&str, MbglValue)]) -> GlobalStateMap {
    entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
}

// gtest: GlobalStateExpression.Parse
#[test]
fn parse() {
    let expr = parse_expression(r#"["global-state", "showLabels"]"#, None).unwrap();
    assert_eq!(Kind::CompoundExpression, expr.get_kind());
    assert_eq!("global-state", expr.get_operator());
    assert_eq!(*expr.get_type(), Type::Value);
    assert!(expr.has(Dependency::GLOBAL_STATE));
}

// gtest: GlobalStateExpression.ParseErrors
#[test]
fn parse_errors() {
    // Missing argument.
    assert!(parse_expression(r#"["global-state"]"#, None).is_none());
    // Too many arguments.
    assert!(parse_expression(r#"["global-state", "a", "b"]"#, None).is_none());
    // Non-string argument.
    assert!(parse_expression(r#"["global-state", 1]"#, None).is_none());
    // The property name must be a string literal, not a sub-expression.
    assert!(parse_expression(r#"["global-state", ["get", "key"]]"#, None).is_none());
    assert!(parse_expression(r#"["global-state", ["concat", "a", "b"]]"#, None).is_none());
    assert!(parse_expression(r#"["global-state", ["literal", "a"]]"#, None).is_none());
}

// gtest: GlobalStateExpression.IsNotConstant
#[test]
fn is_not_constant() {
    let expr = parse_expression(r#"["global-state", "showLabels"]"#, None).unwrap();
    // A global-state expression with constant arguments must not be folded
    // into a literal at parse time.
    assert_ne!(Kind::Literal, expr.get_kind());
    assert!(is_feature_constant(&*expr));
    assert!(is_zoom_constant(&*expr));
    assert!(!is_runtime_constant(&*expr));
}

// gtest: GlobalStateExpression.Evaluate
#[test]
fn evaluate() {
    let expr = parse_expression(r#"["global-state", "size"]"#, None).unwrap();

    let s = state(&[("size", MbglValue::Double(12.0))]);

    let context = EvaluationContext::default().with_global_state(Some(&s));
    assert_eq!(Value::Number(12.0), expr.evaluate(&context).unwrap());

    // Missing property evaluates to null.
    let missing = parse_expression(r#"["global-state", "missing"]"#, None).unwrap();
    assert_eq!(Value::Null, missing.evaluate(&context).unwrap());

    // No global state provided evaluates to null.
    assert_eq!(Value::Null, expr.evaluate(&EvaluationContext::default()).unwrap());
}

// gtest: GlobalStateExpression.EvaluateArray
#[test]
fn evaluate_array() {
    let expr = parse_expression(r#"["global-state", "categories"]"#, None).unwrap();

    let s = state(&[(
        "categories",
        MbglValue::Array(vec![MbglValue::String("restaurant".into()), MbglValue::String("hotel".into())]),
    )]);

    let context = EvaluationContext::default().with_global_state(Some(&s));
    let result = expr.evaluate(&context).unwrap();
    let Value::Array(array) = result else { panic!("array expected") };
    assert_eq!(2, array.len());
    assert_eq!(Value::String("restaurant".into()), array[0]);
    assert_eq!(Value::String("hotel".into()), array[1]);
}

// gtest: GlobalStateExpression.EvaluateInComposition
#[test]
fn evaluate_in_composition() {
    let expr =
        parse_expression(r#"["case", ["to-boolean", ["global-state", "enabled"]], 10.0, 5.0]"#, Some(Type::Number))
            .unwrap();
    assert!(expr.has(Dependency::GLOBAL_STATE));

    let enabled = state(&[("enabled", MbglValue::Bool(true))]);
    let disabled = state(&[("enabled", MbglValue::Bool(false))]);

    let context = EvaluationContext::default().with_global_state(Some(&enabled));
    assert_eq!(Value::Number(10.0), expr.evaluate(&context).unwrap());

    let context = EvaluationContext::default().with_global_state(Some(&disabled));
    assert_eq!(Value::Number(5.0), expr.evaluate(&context).unwrap());

    // Without state, "enabled" is null -> false.
    assert_eq!(Value::Number(5.0), expr.evaluate(&EvaluationContext::default()).unwrap());
}

// PropertyExpressionCapturesGlobalState, PropertyValueConversionKeepsExpression,
// Filter, ColorRamp and PropertyExpressionGlobalStateRefs are in mln_style's
// tests (expression_global_state.rs), FormatSectionOverrideUsesCapturedState
// in mln_style's (tests/format_section_override_test.rs).

// gtest: GlobalStateExpression.CollectGlobalStateRefs
#[test]
fn collect_global_state_refs_() {
    let expr = parse_expression(
        r#"["case", ["to-boolean", ["global-state", "a"]], ["to-number", ["global-state", "b"]], 0]"#,
        Some(Type::Number),
    )
    .unwrap();

    let mut refs = VecSet::new();
    collect_global_state_refs(&*expr, &mut refs);
    assert_eq!(VecSet::from(["a".to_owned(), "b".to_owned()]), refs);

    // Expressions without global-state references collect nothing.
    let plain = parse_expression(r#"["to-number", ["get", "width"]]"#, Some(Type::Number)).unwrap();
    refs.clear();
    collect_global_state_refs(&*plain, &mut refs);
    assert!(refs.is_empty());
}

// gtest: GlobalStateExpression.GlobalStateRefsIntersect
#[test]
fn global_state_refs_intersect_() {
    let refs = VecSet::from(["a".to_owned(), "b".to_owned()]);
    let changed_hit = VecSet::from(["b".to_owned(), "c".to_owned()]);
    let changed_miss = VecSet::from(["c".to_owned(), "d".to_owned()]);

    assert!(global_state_refs_intersect(Some(&refs), Some(&changed_hit)));
    assert!(!global_state_refs_intersect(Some(&refs), Some(&changed_miss)));
    // Unknown refs or unknown changed keys are conservatively a match.
    assert!(global_state_refs_intersect(None, Some(&changed_hit)));
    assert!(global_state_refs_intersect(Some(&refs), None));
}

// gtest: GlobalStateExpression.Serialize
#[test]
fn serialize() {
    let expr = parse_expression(r#"["global-state", "showLabels"]"#, None).unwrap();
    let MbglValue::Array(array) = expr.serialize() else { panic!("array expected") };
    assert_eq!(2, array.len());
    assert_eq!(MbglValue::String("global-state".into()), array[0]);
    assert_eq!(MbglValue::String("showLabels".into()), array[1]);
}
