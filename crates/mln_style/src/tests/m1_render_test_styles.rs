// Not a C++ gtest: checks that every M1 render-test style.json
// (metrics/integration/render-tests, read in place from @maplibre) parses
// into a `Style::Impl` whose background layer evaluates, at the test's zoom,
// through Transitioning -> PossiblyEvaluated to the values the C++ core
// computes. The evaluation mirrors RenderOrchestrator::createRenderTree +
// RenderBackgroundLayer (transition, then evaluate) in MapMode::Static, where
// the transition parameters are `TransitionOptions()` (no transitions), and
// in MapMode::Continuous for the transition test.

use std::path::PathBuf;

use mln_conversion::Convertible;
use mln_style_types::VisibilityType;
use mln_util::chrono::{Duration, TimePoint, milliseconds};
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, EventSeverity};
use mln_util::{Color, UnitBezier};

use super::util::FixtureLog;
use crate::layer::static_immutable_cast;
use crate::layers::BackgroundPaintProperties;
use crate::layers::background_layer::Impl as BackgroundLayerImpl;
use crate::map::zoom_history::ZoomHistory;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::transition_parameters::TransitionParameters;
use crate::style::Impl;
use crate::transition_options::TransitionOptions;

fn render_tests_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/metrics/integration/render-tests")
}

fn load(test: &str) -> Impl {
    let path = render_tests_dir().join(test).join("style.json");
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // C++ style tests use a StubFileSource; sprite requests fail here
    // (background-opacity/color has a sprite), which C++ also counts as
    // loaded.
    let file_source =
        mln_storage::stub_file_source::StubFileSource::new(mln_storage::stub_file_source::ResponseType::Synchronous);
    let failed = |_: &mln_storage::Resource| {
        let mut response = mln_storage::Response::new();
        response.error = Some(Box::new(mln_storage::response::Error::new(
            mln_storage::response::Reason::Other,
            "no sprites in this test",
        )));
        Some(response)
    };
    file_source.set_response(mln_storage::Kind::SpriteJSON, failed);
    file_source.set_response(mln_storage::Kind::SpriteImage, failed);
    let mut style = Impl::with_file_source(
        Some(std::sync::Arc::new(file_source)),
        1.0,
        mln_actor::TaggedScheduler::new(
            mln_actor::scheduler::get_background(),
            mln_util::identity::SimpleIdentity::new(),
        ),
    );
    style.load_json(&json);
    assert!(style.loaded, "{test} did not load");
    assert!(style.is_loaded(), "{test}: style not fully loaded");
    style
}

fn background_impl(style: &Impl, id: &str) -> Immutable<BackgroundLayerImpl> {
    let impls = style.get_layer_impls();
    let impl_ = impls.iter().find(|impl_| impl_.id == id).unwrap_or_else(|| panic!("no layer {id}"));
    assert!(std::ptr::eq(impl_.get_type_info(), BackgroundLayerImpl::static_type_info()));
    static_immutable_cast::<BackgroundLayerImpl>(impl_)
}

/// One RenderOrchestrator update: `layer->transition(transitionParameters)`
/// then `layer->evaluate(evaluationParameters)`.
fn transition_and_evaluate(
    impl_: &BackgroundLayerImpl,
    unevaluated: BackgroundPaintProperties::Unevaluated,
    zoom_history: &mut ZoomHistory,
    zoom: f32,
    now: TimePoint,
    transition: TransitionOptions,
) -> (BackgroundPaintProperties::Unevaluated, BackgroundPaintProperties::PossiblyEvaluated) {
    let unevaluated = impl_.paint.transitioned(&TransitionParameters::new(now, transition), unevaluated);
    zoom_history.update(zoom, &now);
    let parameters = PropertyEvaluationParameters::with_zoom_history(*zoom_history, now, Duration::zero());
    let evaluated = unevaluated.evaluate(&parameters);
    (unevaluated, evaluated)
}

/// A static-mode render of the style's background layer `id` at the style's
/// default zoom.
fn evaluate_static(style: &Impl, id: &str) -> BackgroundPaintProperties::PossiblyEvaluated {
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
        TimePoint::new(milliseconds(1_000_000)),
        TransitionOptions::default(),
    );
    evaluated
}

/// `Interpolator<Color>` with a double t (what the interpolate expression
/// uses): per channel `float(a * (1.0 - t) + b * t)`.
fn lerp_color_f64(a: Color, b: Color, t: f64) -> Color {
    let lerp = |a: f32, b: f32| (a as f64 * (1.0 - t) + b as f64 * t) as f32;
    Color::new_unchecked(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b), lerp(a.a, b.a))
}

/// `Interpolator<Color>` with a float t (what Transitioning uses).
fn lerp_color_f32(a: Color, b: Color, t: f32) -> Color {
    let lerp = |a: f32, b: f32| a * (1.0 - t) + b * t;
    Color::new_unchecked(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b), lerp(a.a, b.a))
}

fn assert_background(evaluated: &BackgroundPaintProperties::PossiblyEvaluated, color: Color, opacity: f32) {
    // Bit-exact, as the renderer consumes these values.
    assert_eq!(evaluated.background_color, color);
    assert_eq!(evaluated.background_opacity.to_bits(), opacity.to_bits());
    assert!(evaluated.background_pattern.from.empty());
    assert!(evaluated.background_pattern.to.empty());
}

#[test]
fn background_color_default() {
    let style = load("background-color/default");
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
    assert_eq!(style.get_layer_impls()[0].visibility, VisibilityType::Visible);
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

/// C++ ignores "colorSpace" in legacy functions (function.cpp interpolates
/// with ColorSpace::RGB), so both colorSpace tests evaluate to the RGB
/// midpoint at zoom 5. (render-tests/background-color/colorSpace-hcl is in
/// metrics/ignores/platform-all.json for that reason.)
fn color_space_expected() -> Color {
    let lower = Color::parse("rgb(118,0,118)").unwrap();
    let upper = Color::parse("rgb(255,155,0)").unwrap();
    // interpolationFactor(base 1, {0, 10}, 5) = 5 / 10 in float.
    let t = (5.0_f32 - 0.0) / (10.0_f32 - 0.0);
    lerp_color_f64(lower, upper, t as f64)
}

#[test]
fn background_color_color_space_hcl() {
    let style = load("background-color/colorSpace-hcl");
    assert_eq!(style.get_default_camera().zoom, Some(5.0));
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, color_space_expected(), 1.0);
}

#[test]
fn background_color_color_space_lab() {
    let style = load("background-color/colorSpace-lab");
    assert_eq!(style.get_default_camera().zoom, Some(5.0));
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, color_space_expected(), 1.0);
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
    let t0 = TimePoint::new(milliseconds(1_000_000));

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

    {
        let white = serde_json::json!("white");
        let mut layer = style.get_layer_mut("background").unwrap();
        assert!(layer.set_property("background-color", &Convertible::from(&white), None).is_none());
    }
    let impl1 = background_impl(&style, "background");
    assert!(!std::sync::Arc::ptr_eq(&impl0, &impl1));

    // What Transitioning::evaluate computes: the prior red->red transition
    // started at t0, the red->white one at t1, both 1000 ms long.
    let ease = |start: TimePoint, now: TimePoint| {
        let t = Duration::float_seconds_div(now - start, milliseconds(1000));
        UnitBezier::new(0.0, 0.0, 0.25, 1.0).solve(t as f64, 0.001) as f32
    };
    let red = Color::parse("red").unwrap();
    let white = Color::parse("white").unwrap();
    let t1 = t0 + milliseconds(16);
    let expected_at = |now: TimePoint| lerp_color_f32(lerp_color_f32(red, red, ease(t0, now)), white, ease(t1, now));

    let (unevaluated, evaluated) =
        transition_and_evaluate(&impl1, unevaluated, &mut zoom_history, zoom, t1, style.get_transition_options());
    // The transition has just begun.
    assert_background(&evaluated, expected_at(t1), 1.0);
    assert!(unevaluated.has_transition());

    let t2 = t1 + milliseconds(500);
    let parameters = PropertyEvaluationParameters::with_zoom_history(zoom_history, t2, Duration::zero());
    let evaluated = unevaluated.evaluate(&parameters);
    assert_background(&evaluated, expected_at(t2), 1.0);
    assert!(unevaluated.has_transition());

    let t3 = t1 + milliseconds(1000);
    let parameters = PropertyEvaluationParameters::with_zoom_history(zoom_history, t3, Duration::zero());
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
    assert_eq!(2, style.get_layer_impls().len());
    assert_eq!(1, style.get_source_impls().len());
    assert_eq!(
        0,
        log.count(EventSeverity::Warning, Event::ParseStyle, "Unsupported layer type! Null factory for type: raster")
    );
    let evaluated = evaluate_static(&style, "background_overlay");
    assert_background(&evaluated, Color::parse("green").unwrap(), 0.5);
}

#[test]
fn background_visibility_none() {
    let style = load("background-visibility/none");
    assert_eq!(style.get_layer_impls()[0].visibility, VisibilityType::None);
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
}

#[test]
fn background_visibility_visible() {
    let style = load("background-visibility/visible");
    assert_eq!(style.get_layer_impls()[0].visibility, VisibilityType::Visible);
    let evaluated = evaluate_static(&style, "background");
    assert_background(&evaluated, Color::black(), 1.0);
}
