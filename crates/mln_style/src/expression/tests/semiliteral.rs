use crate::conversion::Convertible;
use crate::expression::parse::{ParseResult, ParsingContext, ParsingError};
use crate::expression::type_::Type;
use crate::expression::value::Value;
use crate::expression::{EvaluationContext, Functions, evaluate_feature};
use crate::feature::Feature;

fn parse_semiliteral(json: &str, ctx: &mut ParsingContext, property: bool) -> ParseResult {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let value = Convertible::from(&document);
    if property { ctx.parse_layer_property_expression(&value) } else { ctx.parse_expression(&value, None) }
}

// gtest: Semiliteral.ConstantFolding
#[test]
fn constant_folding() {
    let mut ctx = ParsingContext::new(&Functions::with_builtins());
    let result = parse_semiliteral(r#"["semiliteral", [1, ["+", 2, 3]]]"#, &mut ctx, false).unwrap();
    assert!(result.literal_value().is_some());
    assert_eq!(result.get_type(), Type::array_n(Type::Number, 2));
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
        let mut ctx = ParsingContext::new(&Functions::with_builtins());
        assert!(parse_semiliteral(json, &mut ctx, false).is_none(), "{json}");
        assert_ne!(ctx.get_errors(), Vec::<ParsingError>::new());
    }

    for json in [
        r#"["semiliteral", [["get", "x"], ["get", "y"]]]"#,
        r#"["semiliteral", [1]]"#,
        r#"["semiliteral", [1, "x"]]"#,
        r#"["semiliteral", [["zoom"], 1]]"#,
    ] {
        let mut ctx = ParsingContext::with_expected(&Functions::with_builtins(), Type::array_n(Type::Number, 2));
        assert!(parse_semiliteral(json, &mut ctx, true).is_none(), "{json}");
    }
}

// gtest: Semiliteral.FirstError
#[test]
fn first_error() {
    let mut errors = ParsingContext::new(&Functions::with_builtins());
    let failing =
        parse_semiliteral(r#"["semiliteral", [["number", ["get", "x"]], ["error", "second"]]]"#, &mut errors, false)
            .unwrap();
    let feature = Feature::<f64>::default();
    let error = evaluate_feature(&failing, Some(0.0), &feature, None).unwrap_err();
    assert!(error.message().contains("number"));
    assert!(!error.message().contains("second"));
}
