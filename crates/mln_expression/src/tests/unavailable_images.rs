// Port-only: the images `image` expressions record as unavailable
// (image_expression.rs).

use mln_conversion::Convertible;
use mln_vec_map::VecSet;

use crate::expression::{EvaluationContext, Expr};
use crate::image_expression::UnavailableImages;
use crate::parsing_context::ParsingContext;
use crate::type_::Type;

fn coalesce() -> Expr {
    let document: serde_json::Value =
        serde_json::from_str(r#"["coalesce", ["image", "late"], ["image", ""], ["image", "fallback"]]"#).unwrap();
    ParsingContext::with_expected(Type::Image).parse_expression(&Convertible::from(&document), None).unwrap()
}

fn evaluate(expression: &Expr, available: &VecSet<String>, unavailable: Option<&UnavailableImages>) {
    expression
        .evaluate(
            &EvaluationContext::new(0.0).with_available_images(Some(available)).with_unavailable_images(unavailable),
        )
        .unwrap();
}

#[test]
fn records_images_coalesce_passes_over() {
    let expression = coalesce();
    let available: VecSet<String> = ["fallback".to_owned()].into_iter().collect();

    let unavailable = UnavailableImages::new();
    evaluate(&expression, &available, Some(&unavailable));
    evaluate(&expression, &available, Some(&unavailable));
    assert_eq!(unavailable.into_inner().into_iter().collect::<Vec<_>>(), ["late"]);
}

#[test]
fn records_nothing_without_available_images() {
    let expression = coalesce();
    let unavailable = UnavailableImages::new();
    expression.evaluate(&EvaluationContext::new(0.0).with_unavailable_images(Some(&unavailable))).unwrap();
    assert!(unavailable.into_inner().is_empty());
}

#[test]
fn collectors_record_only_their_own_evaluations() {
    let expression = coalesce();
    let (first, second) = (UnavailableImages::new(), UnavailableImages::new());
    evaluate(&expression, &VecSet::new(), Some(&first));
    evaluate(&expression, &VecSet::new(), None);
    assert_eq!(first.into_inner().len(), 2);
    assert!(second.into_inner().is_empty());
}
