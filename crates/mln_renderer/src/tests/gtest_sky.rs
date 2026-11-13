// The sky conversion tests evaluate through `RenderSky`, which is in
// mln_renderer, so they live here rather than with the style's conversion
// tests.

use std::sync::Arc;
use std::time::Duration;

use mln_conversion::Convertible;
use mln_style::conversion::{Converter, Error};
use mln_style::property_value::PropertyValue;
use mln_style::sky::{
    Sky, SkyAtmosphereBlend, SkyColor, SkyFogColor, SkyFogGroundBlend, SkyHorizonBlend, SkyHorizonColor,
    SkyHorizonFogBlend,
};
use mln_style::style_property::Kind;
use mln_style::transition_options::TransitionOptions;
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::Color;
use mln_util::time::Instant;

use crate::render_sky::RenderSky;

fn parse_sky(source: &str, error: &mut Error) -> Option<Sky> {
    let json: serde_json::Value = serde_json::from_str(source).expect("JSON");
    <Sky as Converter>::convert(&Convertible::from(&json), error)
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
    let mut render_sky = RenderSky::new(Arc::clone(&sky.impl_));
    render_sky.evaluate(&PropertyEvaluationParameters::new(0.0));

    let evaluated = render_sky.get_evaluated();
    assert_eq!(Color::new(136.0 / 255.0, 198.0 / 255.0, 252.0 / 255.0, 1.0), *evaluated.get::<SkyColor>());
    assert_eq!(Color::white(), *evaluated.get::<SkyHorizonColor>());
    assert_eq!(Color::white(), *evaluated.get::<SkyFogColor>());
    assert_float_eq(0.5, *evaluated.get::<SkyFogGroundBlend>());
    assert_float_eq(0.8, *evaluated.get::<SkyHorizonFogBlend>());
    assert_float_eq(0.8, *evaluated.get::<SkyHorizonBlend>());
    assert_float_eq(0.8, *evaluated.get::<SkyAtmosphereBlend>());
}

// gtest: StyleConversion.SkySetAndGet
#[test]
fn sky_set_and_get() {
    let mut sky = Sky::new();
    sky.set_sky_color(PropertyValue::Constant(Color::red()));
    sky.set_horizon_color(PropertyValue::Constant(Color::green()));
    sky.set_fog_color(PropertyValue::Constant(Color::blue()));
    sky.set_fog_ground_blend(PropertyValue::Constant(0.1));
    sky.set_horizon_fog_blend(PropertyValue::Constant(0.2));
    sky.set_sky_horizon_blend(PropertyValue::Constant(0.3));
    sky.set_atmosphere_blend(PropertyValue::Constant(0.4));

    assert_eq!(Color::red(), *sky.get_sky_color().as_constant());
    assert_eq!(Color::green(), *sky.get_horizon_color().as_constant());
    assert_eq!(Color::blue(), *sky.get_fog_color().as_constant());
    assert_float_eq(0.1, *sky.get_fog_ground_blend().as_constant());
    assert_float_eq(0.2, *sky.get_horizon_fog_blend().as_constant());
    assert_float_eq(0.3, *sky.get_sky_horizon_blend().as_constant());
    assert_float_eq(0.4, *sky.get_atmosphere_blend().as_constant());
    assert_eq!(Kind::Expression, sky.get_property("sky-color").get_kind());
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

    assert_eq!(Color::red(), *sky.get_horizon_color().as_constant());
    assert_eq!(Color::blue(), *sky.get_fog_color().as_constant());
    assert_float_eq(0.1, *sky.get_fog_ground_blend().as_constant());
    assert_float_eq(0.2, *sky.get_horizon_fog_blend().as_constant());
    assert_float_eq(0.3, *sky.get_sky_horizon_blend().as_constant());
    assert_float_eq(0.4, *sky.get_atmosphere_blend().as_constant());
    assert_eq!(Some(Duration::from_millis(1000)), sky.get_sky_color_transition().duration);
    assert_eq!(Some(Duration::from_millis(20)), sky.get_sky_color_transition().delay);

    let mut transition_source = Sky::new();
    transition_source.set_atmosphere_blend(PropertyValue::Constant(0.4));
    let mut transitioning = RenderSky::new(Arc::clone(&transition_source.impl_));
    let mut changed = transition_source.mutable_impl();
    changed.properties.get_mut::<SkyAtmosphereBlend>().value = PropertyValue::Constant(0.9);
    transitioning.impl_ = Arc::new(changed);
    let start = Instant::now();
    transitioning.transition(&TransitionParameters::new(
        Some(start),
        TransitionOptions { duration: Some(Duration::from_millis(100)), ..TransitionOptions::default() },
    ));
    assert!(transitioning.has_transition());

    let mut evaluation = PropertyEvaluationParameters::new(0.0);
    evaluation.now = Some(start);
    transitioning.evaluate(&evaluation);
    assert_float_eq(0.4, *transitioning.get_evaluated().get::<SkyAtmosphereBlend>());

    evaluation.now = Some(start + Duration::from_millis(100));
    transitioning.evaluate(&evaluation);
    assert_float_eq(0.9, *transitioning.get_evaluated().get::<SkyAtmosphereBlend>());
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
    assert!(sky.get_atmosphere_blend().is_expression());
    assert!(!sky.get_atmosphere_blend().as_expression().is_zoom_constant());

    let mut render_sky = RenderSky::new(Arc::clone(&sky.impl_));
    render_sky.evaluate(&PropertyEvaluationParameters::new(5.0));
    assert_float_eq(0.5, *render_sky.get_evaluated().get::<SkyAtmosphereBlend>());
    assert_eq!(Color::new(0.5, 0.5, 0.5, 1.0), *render_sky.get_evaluated().get::<SkyColor>());

    render_sky.evaluate(&PropertyEvaluationParameters::new(10.0));
    assert_float_eq(1.0, *render_sky.get_evaluated().get::<SkyAtmosphereBlend>());
    assert_eq!(Color::white(), *render_sky.get_evaluated().get::<SkyColor>());
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
