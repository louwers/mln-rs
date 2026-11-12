// Ports test/style/expression/semiliteral.test.cpp.

use feature::Feature;
use mln_conversion::Convertible;

use crate::expression::{EvaluationContext, Kind, evaluate_feature};
use crate::parsing_context::{ParseResult, ParsingContext, ParsingError};
use crate::type_::Type;
use crate::value::Value;

fn parse_semiliteral(json: &str, ctx: &mut ParsingContext, property: bool) -> ParseResult {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let value = Convertible::from(&document);
    if property { ctx.parse_layer_property_expression(&value) } else { ctx.parse_expression(&value, None) }
}

// gtest: Semiliteral.ConstantFolding
#[test]
fn constant_folding() {
    let mut ctx = ParsingContext::new();
    let result = parse_semiliteral(r#"["semiliteral", [1, ["+", 2, 3]]]"#, &mut ctx, false).unwrap();
    assert_eq!(result.get_kind(), Kind::Literal);
    assert_eq!(*result.get_type(), Type::array_n(Type::Number, 2));
    assert_eq!(
        result.evaluate(&EvaluationContext::default()).unwrap(),
        Value::Array(vec![Value::Number(1.0), Value::Number(5.0)])
    );
}

// gtest: Semiliteral.Invalid
#[test]
fn invalid() {
    for json in [
        r#"["semiliteral"]"#,
        r#"["semiliteral", [], []]"#,
        r#"["semiliteral", [["unknown"]]]"#,
        r#"["semiliteral", [[1, 2]]]"#,
        r#"["semiliteral", [{}]]"#,
    ] {
        let mut ctx = ParsingContext::new();
        assert!(parse_semiliteral(json, &mut ctx, false).is_none(), "{json}");
        assert_ne!(ctx.get_errors(), Vec::<ParsingError>::new());
    }

    for json in [
        r#"["semiliteral", [["get", "x"], ["get", "y"]]]"#,
        r#"["semiliteral", [1]]"#,
        r#"["semiliteral", [1, "x"]]"#,
        r#"["semiliteral", [["zoom"], 1]]"#,
    ] {
        let mut ctx = ParsingContext::with_expected(Type::array_n(Type::Number, 2));
        assert!(parse_semiliteral(json, &mut ctx, true).is_none(), "{json}");
    }
}

// gtest: Semiliteral.FirstError
#[test]
fn first_error() {
    let mut errors = ParsingContext::new();
    let failing =
        parse_semiliteral(r#"["semiliteral", [["number", ["get", "x"]], ["error", "second"]]]"#, &mut errors, false)
            .unwrap();
    let feature = Feature::<f64>::default();
    let error = evaluate_feature(&*failing, Some(0.0), &feature, None).unwrap_err();
    assert!(error.message.contains("number"));
    assert!(!error.message.contains("second"));
}
