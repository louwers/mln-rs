// The sky conversion tests evaluate through `RenderSky`, which is in
// mln_engine, so they live here rather than with the style's conversion
// tests.

use std::sync::Arc;
use std::time::Duration;

use feature::{Value, ValueObject};
use mln_style::color::Color;
use mln_style::conversion::{Convertible, Error};
use mln_style::expression::Functions;
use mln_style::property::table::Kind;
use mln_style::property::transition::TransitionOptions;
use mln_style::property::value::PropertyValue;
use mln_style::sky::{Sky, convert_sky};
use mln_style::time::Instant;
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use serde_json::json;

use crate::render::sky::RenderSky;

fn parse_sky(source: &str, error: &mut Error) -> Option<Sky> {
    let json: serde_json::Value = serde_json::from_str(source).expect("JSON");
    convert_sky(&Convertible::from(&json), &Functions::with_builtins(), error)
}

/// Sets the sky's property `name` from style JSON.
#[track_caller]
fn set(sky: &mut Sky, name: &str, value: &serde_json::Value) {
    let error = sky.set_property(name, &Convertible::from(value), &Functions::with_builtins());
    assert!(error.is_none(), "{name}: {error:?}");
}

/// gtest `EXPECT_FLOAT_EQ`: within 4 ULPs.
#[track_caller]
fn assert_float_eq(expected: f32, actual: f32) {
    let diff = (i64::from(expected.to_bits()) - i64::from(actual.to_bits())).abs();
    assert!(diff <= 4, "expected {expected}, got {actual}");
}

// gtest: StyleConversion.SkyDefaults
#[test]
fn sky_defaults() {
    let sky = Sky::new();
    let mut render_sky = RenderSky::new(Arc::new(sky));
    render_sky.evaluate(&PropertyEvaluationParameters::new(0.0));

    let evaluated = render_sky.evaluated();
    assert_eq!(Color::new(136.0 / 255.0, 198.0 / 255.0, 252.0 / 255.0, 1.0), *evaluated.sky_color.uniform());
    assert_eq!(Color::white(), *evaluated.sky_horizon_color.uniform());
    assert_eq!(Color::white(), *evaluated.sky_fog_color.uniform());
    assert_float_eq(0.5, *evaluated.sky_fog_ground_blend.uniform());
    assert_float_eq(0.8, *evaluated.sky_horizon_fog_blend.uniform());
    assert_float_eq(0.8, *evaluated.sky_horizon_blend.uniform());
    assert_float_eq(0.8, *evaluated.sky_atmosphere_blend.uniform());
}

// gtest: StyleConversion.SkySetAndGet
#[test]
fn sky_set_and_get() {
    let mut sky = Sky::new();
    set(&mut sky, "sky-color", &json!("rgba(255,0,0,1)"));
    set(&mut sky, "horizon-color", &json!("rgba(0,255,0,1)"));
    set(&mut sky, "fog-color", &json!("rgba(0,0,255,1)"));
    set(&mut sky, "fog-ground-blend", &json!(0.1));
    set(&mut sky, "horizon-fog-blend", &json!(0.2));
    set(&mut sky, "sky-horizon-blend", &json!(0.3));
    set(&mut sky, "atmosphere-blend", &json!(0.4));

    let properties = &sky.properties;
    assert_eq!(Color::red(), *properties.sky_color.value.as_constant().unwrap());
    assert_eq!(Color::green(), *properties.sky_horizon_color.value.as_constant().unwrap());
    assert_eq!(Color::blue(), *properties.sky_fog_color.value.as_constant().unwrap());
    assert_float_eq(0.1, *properties.sky_fog_ground_blend.value.as_constant().unwrap());
    assert_float_eq(0.2, *properties.sky_horizon_fog_blend.value.as_constant().unwrap());
    assert_float_eq(0.3, *properties.sky_horizon_blend.value.as_constant().unwrap());
    assert_float_eq(0.4, *properties.sky_atmosphere_blend.value.as_constant().unwrap());
    assert_eq!(Kind::Constant, sky.get_property("sky-color").get_kind());
    assert_eq!(Kind::Undefined, sky.get_property("unknown").get_kind());
}

// gtest: StyleConversion.SkyConstantsAndTransitions
#[test]
fn sky_constants_and_transitions() {
    let mut error = Error::default();
    let sky = parse_sky(
        r##"{
        "sky-color": "#123456",
        "horizon-color": "red",
        "fog-color": "blue",
        "fog-ground-blend": 0.1,
        "horizon-fog-blend": 0.2,
        "sky-horizon-blend": 0.3,
        "atmosphere-blend": 0.4,
        "sky-color-transition": {"duration": 1000, "delay": 20}
    }"##,
        &mut error,
    )
    .expect("sky");

    let properties = &sky.properties;
    assert_eq!(Color::red(), *properties.sky_horizon_color.value.as_constant().unwrap());
    assert_eq!(Color::blue(), *properties.sky_fog_color.value.as_constant().unwrap());
    assert_float_eq(0.1, *properties.sky_fog_ground_blend.value.as_constant().unwrap());
    assert_float_eq(0.2, *properties.sky_horizon_fog_blend.value.as_constant().unwrap());
    assert_float_eq(0.3, *properties.sky_horizon_blend.value.as_constant().unwrap());
    assert_float_eq(0.4, *properties.sky_atmosphere_blend.value.as_constant().unwrap());
    let transition = sky.get_property("sky-color-transition");
    assert_eq!(Kind::Transition, transition.get_kind());
    assert_eq!(
        &Value::Object(ValueObject::from_iter([
            ("duration".to_owned(), Value::Int(1000)),
            ("delay".to_owned(), Value::Int(20)),
        ])),
        transition.get_value()
    );

    let mut transition_source = Sky::new();
    set(&mut transition_source, "atmosphere-blend", &json!(0.4));
    let mut changed = transition_source.clone();
    let mut transitioning = RenderSky::new(Arc::new(transition_source));
    changed.properties.sky_atmosphere_blend.value = PropertyValue::Constant(0.9);
    transitioning.sky = Arc::new(changed);
    let start = Instant::now();
    transitioning.transition(&TransitionParameters::new(
        Some(start),
        TransitionOptions { duration: Some(Duration::from_millis(100)), ..TransitionOptions::default() },
    ));
    assert!(transitioning.has_transition());

    let mut evaluation = PropertyEvaluationParameters::new(0.0);
    evaluation.now = Some(start);
    transitioning.evaluate(&evaluation);
    assert_float_eq(0.4, *transitioning.evaluated().sky_atmosphere_blend.uniform());

    evaluation.now = Some(start + Duration::from_millis(100));
    transitioning.evaluate(&evaluation);
    assert_float_eq(0.9, *transitioning.evaluated().sky_atmosphere_blend.uniform());
    assert!(!transitioning.has_transition());
}

// gtest: StyleConversion.SkyZoomExpressions
#[test]
fn sky_zoom_expressions() {
    let mut error = Error::default();
    let sky = parse_sky(
        r#"{
        "atmosphere-blend": ["interpolate", ["linear"], ["zoom"], 0, 0, 10, 1],
        "sky-color": ["interpolate", ["linear"], ["zoom"], 0, "black", 10, "white"]
    }"#,
        &mut error,
    )
    .unwrap_or_else(|| panic!("{}", error.message));
    let atmosphere_blend = &sky.properties.sky_atmosphere_blend.value;
    assert!(atmosphere_blend.is_expression());
    assert!(!atmosphere_blend.as_expression().unwrap().is_zoom_constant());

    let mut render_sky = RenderSky::new(Arc::new(sky));
    render_sky.evaluate(&PropertyEvaluationParameters::new(5.0));
    assert_float_eq(0.5, *render_sky.evaluated().sky_atmosphere_blend.uniform());
    assert_eq!(Color::new(0.5, 0.5, 0.5, 1.0), *render_sky.evaluated().sky_color.uniform());

    render_sky.evaluate(&PropertyEvaluationParameters::new(10.0));
    assert_float_eq(1.0, *render_sky.evaluated().sky_atmosphere_blend.uniform());
    assert_eq!(Color::white(), *render_sky.evaluated().sky_color.uniform());
}

// gtest: StyleConversion.InvalidSky
#[test]
fn invalid_sky() {
    let mut error = Error::default();

    assert!(parse_sky("[]", &mut error).is_none());
    assert_eq!("sky must be an object", error.message);

    assert!(parse_sky(r#"{"atmosphere-blend":"opaque"}"#, &mut error).is_none());
    assert_eq!("value must be a number", error.message);

    assert!(parse_sky(r#"{"sky-color":5}"#, &mut error).is_none());
    assert_eq!("value must be a string", error.message);
}
