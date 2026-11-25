use crate::conversion::Convertible;
use crate::expression::parse::ParsingContext;
use crate::expression::value::{Value, to_mbgl_value};
use crate::expression::{EvaluationContext, Functions};
use crate::types::VerticalAlign;

fn evaluate(document: &Convertible<'_>) -> Value {
    let mut ctx = ParsingContext::new(&Functions::with_builtins());
    let expression = ctx.parse_expression(document, None).unwrap_or_else(|| panic!("{:?}", ctx.get_errors()));
    expression.evaluate(&EvaluationContext::default()).unwrap()
}

#[test]
fn vertical_align_applies_to_text_and_image_sections_and_round_trips() {
    let document = serde_json::json!([
        "format",
        ["image", "icon"], {"vertical-align": "top"},
        "a", {"vertical-align": "center"},
        "b", {}
    ]);
    let value = evaluate(&Convertible::from(&document));
    let Value::Formatted(formatted) = &value else { panic!("{value:?}") };
    let aligns: Vec<_> = formatted.sections.iter().map(|section| section.vertical_align).collect();
    assert_eq!(aligns, [Some(VerticalAlign::Top), Some(VerticalAlign::Center), None]);

    // A formatted value serializes as the expression that makes it.
    let serialized = to_mbgl_value(&value);
    assert_eq!(evaluate(&Convertible::from(&serialized)), value);
}
