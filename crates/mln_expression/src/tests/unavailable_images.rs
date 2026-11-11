// Port-only: the images `image` expressions record as unavailable
// (image_expression.rs).

use mln_conversion::Convertible;
use mln_vec_map::VecSet;

use crate::expression::{EvaluationContext, Expression};
use crate::image_expression::UnavailableImages;
use crate::parsing_context::ParsingContext;
use crate::type_::Type;

fn coalesce() -> Box<dyn Expression> {
    let document: serde_json::Value =
        serde_json::from_str(r#"["coalesce", ["image", "late"], ["image", ""], ["image", "fallback"]]"#).unwrap();
    ParsingContext::with_expected(Type::Image).parse_expression(&Convertible::from(&document), None).unwrap()
}

fn evaluate(expression: &dyn Expression, available: &VecSet<String>) {
    expression.evaluate(&EvaluationContext::new(0.0).with_available_images(Some(available))).unwrap();
}

#[test]
fn records_images_coalesce_passes_over() {
    let expression = coalesce();
    let available: VecSet<String> = ["fallback".to_owned()].into_iter().collect();

    let recorder = UnavailableImages::start();
    evaluate(&*expression, &available);
    assert_eq!(recorder.finish().into_iter().collect::<Vec<_>>(), ["late"]);
}

#[test]
fn records_nothing_outside_a_recorder() {
    let expression = coalesce();
    evaluate(&*expression, &VecSet::new());

    let recorder = UnavailableImages::start();
    assert!(recorder.finish().is_empty());
}

#[test]
fn records_nothing_without_available_images() {
    let expression = coalesce();
    let recorder = UnavailableImages::start();
    expression.evaluate(&EvaluationContext::new(0.0)).unwrap();
    assert!(recorder.finish().is_empty());
}

#[test]
fn an_inner_recorder_hides_its_images_from_the_outer() {
    let expression = coalesce();
    let outer = UnavailableImages::start();
    {
        let inner = UnavailableImages::start();
        evaluate(&*expression, &VecSet::new());
        assert_eq!(inner.finish().len(), 2);
    }
    assert!(outer.finish().is_empty());
}
