// Not a C++ gtest: checks that every M1 render-test style.json
// (metrics/integration/render-tests, read in place from @maplibre) parses
// into a `Style` whose background layer evaluates, at the test's zoom,
// through Transitioning -> PossiblyEvaluated to the values the C++ core
// computes. The evaluation mirrors RenderOrchestrator::createRenderTree +
// RenderBackgroundLayer (transition, then evaluate) in MapMode::Static, where
// the transition parameters are `TransitionOptions()` (no transitions), and
// in MapMode::Continuous for the transition test.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::util::FixtureLog;
use crate::color::Color;
use crate::expression::Functions;
use crate::layer::LayerStyle;
use crate::layers::{Background, BackgroundPaintProperties};
use crate::property::evaluation::{PropertyEvaluationParameters, ZoomHistory};
use crate::property::transition::{TransitionOptions, TransitionParameters};
use crate::registry::StyleRegistry;
use crate::style::Style;
use crate::time::Instant;
use crate::types::VisibilityType;
use crate::unitbezier::UnitBezier;

fn render_tests_dir() -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("testdata/metrics/integration/render-tests")
}

fn load(test: &str) -> Style {
    let path = render_tests_dir().join(test).join("style.json");
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Style::from_json(&StyleRegistry::with_builtins(), &json).unwrap_or_else(|e| panic!("{test}: {e}"))
}

fn background_impl(style: &Style, id: &str) -> Arc<LayerStyle<Background>> {
    let layer = style.get_layer(id).unwrap_or_else(|| panic!("no layer {id}"));
    let impl_ = layer.style();
    assert_eq!(impl_.info().name, "background");
    LayerStyle::recover(impl_).unwrap()
}

/// One `RenderOrchestrator` update: `layer->transition(transitionParameters)`
/// then `layer->evaluate(evaluationParameters)`.
fn transition_and_evaluate(
    impl_: &LayerStyle<Background>,
    unevaluated: BackgroundPaintProperties::Unevaluated,
    zoom_history: &mut ZoomHistory,
    zoom: f32,
    now: Instant,
    transition: TransitionOptions,
) -> (BackgroundPaintProperties::Unevaluated, BackgroundPaintProperties::PossiblyEvaluated) {
    let mut unevaluated = impl_.paint.transitioned(&TransitionParameters::new(Some(now), transition), unevaluated);
    zoom_history.update(zoom, Some(now));
    let parameters = PropertyEvaluationParameters::with_zoom_history(*zoom_history, Some(now), Duration::ZERO);
    let evaluated = unevaluated.evaluate(&parameters);
    (unevaluated, evaluated)
}

/// A static-mode render of the style's background layer `id` at the style's
/// default zoom.
fn evaluate_static(style: &Style, id: &str) -> BackgroundPaintProperties::PossiblyEvaluated {
    let impl_ = background_impl(style, id);
    let zoom = style.get_default_camera().zoom.unwrap() as f32;
    let mut zoom_history = ZoomHistory::default();
    // RenderBackgroundLayer(impl): unevaluated(impl.paint.untransitioned()).
    let unevaluated = impl_.paint.untransitioned();
    let (_, evaluated) = transition_and_evaluate(
        &impl_,
        unevaluated,
        &mut zoom_history,
        zoom,
        Instant::now(),
        TransitionOptions::default(),
    );
    evaluated
}

/// Interpolates colors with a float t, as transitions do.
fn lerp_color_f32(a: Color, b: Color, t: f32) -> Color {
    let lerp = |a: f32, b: f32| a * (1.0 - t) + b * t;
    Color::new_unchecked(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b), lerp(a.a, b.a))
}

fn assert_background(evaluated: &BackgroundPaintProperties::PossiblyEvaluated, color: Color, opacity: f32) {
    // Bit-exact, as the renderer consumes these values.
    assert_eq!(*evaluated.background_color.uniform(), color);
    assert_eq!(evaluated.background_opacity.uniform().to_bits(), opacity.to_bits());
    assert!(evaluated.background_pattern.faded().from.empty());
    assert!(evaluated.background_pattern.faded().to.empty());
}

#[test]
fn background_color_default() {
    let style = load("background-color/default");
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
    assert_eq!(style.get_layer_list().first().unwrap().get_visibility(), VisibilityType::Visible);
}

#[test]
fn background_color_literal() {
    let style = load("background-color/literal");
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::parse("red").unwrap(), 1.0);
}

#[test]
fn background_color_function() {
    let style = load("background-color/function");
    // {"stops": [[0, "blue"], [10, "white"]]} at the default zoom 0.
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::parse("blue").unwrap(), 1.0);
}

/// The colorSpace tests' stops, interpolated by `operator` at their zoom, 5.
fn color_space_expected(operator: &str) -> Color {
    let expression = crate::expression::parse::create_expression_from_str(
        &format!(r#"["{operator}", ["linear"], ["zoom"], 0, "rgb(118,0,118)", 10, "rgb(255,155,0)"]"#),
        &Functions::with_builtins(),
    )
    .unwrap();
    crate::property::expression::PropertyExpression::<Color>::from_expr(expression)
        .evaluate(&crate::expression::EvaluationContext::new(5.0), Color::black())
}

#[test]
fn background_color_color_space_hcl() {
    let style = load("background-color/colorSpace-hcl");
    assert_eq!(style.get_default_camera().zoom, Some(5.0));
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, color_space_expected("interpolate-hcl"), 1.0);
}

#[test]
fn background_color_color_space_lab() {
    let style = load("background-color/colorSpace-lab");
    assert_eq!(style.get_default_camera().zoom, Some(5.0));
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, color_space_expected("interpolate-lab"), 1.0);
}

#[test]
fn background_color_transition() {
    // operations: setPaintProperty background-color white, wait 0, wait 500.
    // The runner waits in real time; this checks the evaluation at fixed
    // times in MapMode::Continuous (where the style's transition options
    // apply): the first frame at t0, the property change at t1, a frame at
    // t1 + 500 ms and one after the 1000 ms transition.
    let mut style = load("background-color/transition");
    let zoom = 0.0_f32;
    let mut zoom_history = ZoomHistory::default();
    let t0 = Instant::now();

    let impl0 = background_impl(&style, "background");
    let (unevaluated, evaluated) = transition_and_evaluate(
        &impl0,
        impl0.paint.untransitioned(),
        &mut zoom_history,
        zoom,
        t0,
        style.get_transition_options(),
    );
    assert_background(&evaluated, Color::parse("red").unwrap(), 1.0);

    let white = serde_json::json!("white");
    let edit = crate::Edit::SetLayerProperty {
        layer: "background".to_owned(),
        name: "background-color".to_owned(),
        value: white,
    };
    style.apply(edit).unwrap();
    let impl1 = background_impl(&style, "background");
    assert!(!Arc::ptr_eq(&impl0, &impl1));

    // What Transitioning::evaluate computes: the prior red->red transition
    // started at t0, the red->white one at t1, both 1000 ms long.
    let ease = |start: Instant, now: Instant| {
        let t = (now - start).div_duration_f32(Duration::from_millis(1000));
        UnitBezier::new(0.0, 0.0, 0.25, 1.0).solve(f64::from(t), 0.001) as f32
    };
    let red = Color::parse("red").unwrap();
    let white = Color::parse("white").unwrap();
    let t1 = t0 + Duration::from_millis(16);
    let expected_at = |now: Instant| lerp_color_f32(lerp_color_f32(red, red, ease(t0, now)), white, ease(t1, now));

    let (mut unevaluated, evaluated) =
        transition_and_evaluate(&impl1, unevaluated, &mut zoom_history, zoom, t1, style.get_transition_options());
    // The transition has just begun.
    assert_background(&evaluated, expected_at(t1), 1.0);
    assert!(unevaluated.has_transition());

    let t2 = t1 + Duration::from_millis(500);
    let parameters = PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(t2), Duration::ZERO);
    let evaluated = unevaluated.evaluate(&parameters);
    assert_background(&evaluated, expected_at(t2), 1.0);
    assert!(unevaluated.has_transition());

    let t3 = t1 + Duration::from_millis(1000);
    let parameters = PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(t3), Duration::ZERO);
    let evaluated = unevaluated.evaluate(&parameters);
    assert_background(&evaluated, white, 1.0);
    assert!(!unevaluated.has_transition());
}

#[test]
fn background_opacity_color() {
    let style = load("background-opacity/color");
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::parse("red").unwrap(), 0.5);
}

#[test]
fn background_opacity_overlay() {
    // The raster source converts (sources, M3/M4) and the raster layer type
    // is registered (generated layers), so both layers and the source parse.
    let log = FixtureLog::new();
    let style = load("background-opacity/overlay");
    assert_eq!(2, style.get_layer_list().len());
    assert_eq!(1, style.get_sources().len());
    assert_eq!(
        0,
        log.count(
            log::Level::Warn,
            crate::log_target::PARSE_STYLE,
            "Unsupported layer type! Null factory for type: raster"
        )
    );
    let evaluated = evaluate_static(&style, "background_overlay");
    assert_background(&evaluated, Color::parse("green").unwrap(), 0.5);
}

#[test]
fn background_visibility_none() {
    let style = load("background-visibility/none");
    assert_eq!(style.get_layer_list().first().unwrap().get_visibility(), VisibilityType::None);
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
}

#[test]
fn background_visibility_visible() {
    let style = load("background-visibility/visible");
    assert_eq!(style.get_layer_list().first().unwrap().get_visibility(), VisibilityType::Visible);
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
}
