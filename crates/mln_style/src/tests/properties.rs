use std::sync::{Arc, LazyLock};
use std::time::Duration;

use feature::Value;
use mln_util::time::Instant;
use mln_vec_map::VecSet;

use super::util::parse_json;
use crate::conversion::{Convertible, Error, convert_property_value};
use crate::evaluated::Evaluated;
use crate::expression::{EvaluationContext, Functions, GlobalStateMap, Image, dsl};
use crate::light::LightProperties;
use crate::map::zoom_history::ZoomHistory;
use crate::properties::{Transitionable, Transitioning};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::transition_parameters::{GlobalStateChange, TransitionParameters};
use crate::transition_options::TransitionOptions;

/// The time the tests' transitions start at.
fn start() -> Instant {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    *START
}

fn evaluate(property: &mut Transitioning<PropertyValue<f32>>, delta: Duration) -> f32 {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);

    *property.evaluate(Evaluated::plain, &parameters, 0.0_f32).uniform()
}

/// gtest `ASSERT_FLOAT_EQ`: within 4 ULPs.
fn assert_float_eq(expected: f32, actual: f32) {
    let diff = (i64::from(expected.to_bits()) - i64::from(actual.to_bits())).abs();
    assert!(diff <= 4, "expected {expected}, got {actual}");
}

// gtest: TransitioningPropertyValue.EvaluateDefaultValue
#[test]
fn evaluate_default_value() {
    let mut property = Transitioning::<PropertyValue<f32>>::default();
    assert_eq!(0.0_f32, evaluate(&mut property, Duration::ZERO));
}

// gtest: TransitioningPropertyValue.EvaluateUntransitionedConstant
#[test]
fn evaluate_untransitioned_constant() {
    let mut property = Transitioning::with_prior(
        PropertyValue::Constant(1.0_f32),
        Transitioning::default(),
        &TransitionOptions::default(),
        Some(start()),
    );

    assert_eq!(1.0_f32, evaluate(&mut property, Duration::ZERO));
}

// gtest: TransitioningPropertyValue.EvaluateTransitionedConstantWithoutDelay
#[test]
fn evaluate_transitioned_constant_without_delay() {
    let transition = TransitionOptions { duration: Some(Duration::from_millis(1000)), ..TransitionOptions::default() };

    let t0 = Transitioning::with_prior(
        PropertyValue::Constant(0.0_f32),
        Transitioning::default(),
        &TransitionOptions::default(),
        Some(start()),
    );

    let mut t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));

    assert_float_eq(0.0, evaluate(&mut t1, Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate(&mut t1, Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate(&mut t1, Duration::from_millis(1500)));
}

// gtest: TransitioningPropertyValue.EvaluateTransitionedConstantWithDelay
#[test]
fn evaluate_transitioned_constant_with_delay() {
    let transition = TransitionOptions {
        delay: Some(Duration::from_millis(1000)),
        duration: Some(Duration::from_millis(1000)),
        ..TransitionOptions::default()
    };

    let t0 = Transitioning::with_prior(
        PropertyValue::Constant(0.0_f32),
        Transitioning::default(),
        &TransitionOptions::default(),
        Some(start()),
    );

    let mut t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));

    assert_float_eq(0.0, evaluate(&mut t1, Duration::from_millis(0)));
    assert_float_eq(0.0, evaluate(&mut t1, Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate(&mut t1, Duration::from_millis(612)));
    assert_float_eq(0.823_099, evaluate(&mut t1, Duration::from_millis(1500)));
    assert_float_eq(1.0, evaluate(&mut t1, Duration::from_millis(2500)));
}

fn evaluate_data_expression(property: &mut Transitioning<PropertyValue<f32>>, delta: Duration) -> Evaluated<f32> {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);

    property.evaluate(Evaluated::plain, &parameters, 0.0_f32)
}

// gtest: TransitioningDataDrivenPropertyValue.Evaluate
#[test]
fn transitioning_data_driven_property_value_evaluate() {
    let transition = TransitionOptions {
        delay: Some(Duration::from_millis(1000)),
        duration: Some(Duration::from_millis(1000)),
        ..TransitionOptions::default()
    };

    let mut t0 = Transitioning::with_prior(
        PropertyValue::Constant(0.0_f32),
        Transitioning::default(),
        &TransitionOptions::default(),
        Some(start()),
    );

    let expression = PropertyExpression::<f32>::from_expr(dsl::number(dsl::get_str("property_name"), None));

    let mut t1 =
        Transitioning::with_prior(PropertyValue::Expression(expression), t0.clone(), &transition, Some(start()));

    assert!(evaluate_data_expression(&mut t0, Duration::from_millis(0)).is_constant());
    assert!(
        !evaluate_data_expression(&mut t1, Duration::from_millis(0)).is_constant(),
        "A paint property transition to a data-driven evaluates immediately to the final value (see \
         https://github.com/mapbox/mapbox-gl-native/issues/8237)."
    );
}

#[test]
fn transition_completes_and_drops_prior() {
    // Not a C++ test: checks the `mutable prior` reset that hasTransition()
    // observes (properties.hpp).
    let transition = TransitionOptions { duration: Some(Duration::from_millis(1000)), ..TransitionOptions::default() };
    let t0 = Transitioning::new(PropertyValue::Constant(0.0_f32));
    let mut t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));
    assert!(t1.has_transition());
    assert_float_eq(0.823_099, evaluate(&mut t1, Duration::from_millis(500)));
    assert!(t1.has_transition());
    assert_eq!(1.0, evaluate(&mut t1, Duration::from_millis(1000)));
    assert!(!t1.has_transition());
}

/// `PossiblyEvaluated::evaluate::<P>(z, feature, ...)`, as the layouts call
/// it.
#[test]
fn possibly_evaluated_evaluate_per_feature() {
    use mln_style_types::{AlignmentType, TextJustifyType};
    use mln_tile_id::CanonicalTileID;

    use crate::expression::dsl::{get_str, number};
    use crate::property_expression::PropertyExpression;
    use crate::tests::fixture_layer::DotLayoutProperties;
    use crate::tests::util::{StubGeometryTileFeature, property_map};

    let feature = StubGeometryTileFeature::new(property_map(&[("r", Value::Double(7.5))]));

    // A constant.
    let mut layout = DotLayoutProperties::PossiblyEvaluated {
        dot_size: Evaluated::Constant(3.0),
        ..DotLayoutProperties::PossiblyEvaluated::default()
    };
    assert_eq!(3.0, layout.dot_size.evaluate(&EvaluationContext::with_zoom_feature(10.0, Some(&feature))));
    // A feature expression, with the property's default for a missing value.
    layout.dot_size =
        Evaluated::PerFeature { expression: PropertyExpression::from_expr(number(get_str("r"), None)), default: 5.0 };
    assert_eq!(7.5, layout.dot_size.evaluate(&EvaluationContext::with_zoom_feature(10.0, Some(&feature))));
    let other = StubGeometryTileFeature::new(property_map(&[]));
    assert_eq!(5.0, layout.dot_size.evaluate(&EvaluationContext::with_zoom_feature(10.0, Some(&other))));
    // A plain (non-data-driven) property.
    layout.dot_alignment = Evaluated::Constant(AlignmentType::Map);
    assert_eq!(
        AlignmentType::Map,
        layout.dot_alignment.evaluate(&EvaluationContext::with_zoom_feature(10.0, Some(&feature)))
    );

    assert_eq!(
        TextJustifyType::default(),
        layout.dot_justify.evaluate(
            &EvaluationContext::with_zoom_feature(10.0, Some(&feature))
                .with_canonical_tile_id(Some(&CanonicalTileID::new(0, 0, 0)))
        )
    );
}

fn global_state_value(key: &str) -> PropertyValue<f32> {
    let json = parse_json(&format!(r#"["global-state", "{key}"]"#));
    let mut error = Error::default();
    let value =
        convert_property_value::<f32>(&Convertible::from(&json), &mut error, false, false, &Functions::with_builtins());
    value.unwrap_or_else(|| panic!("{}", error.message))
}

fn width_state(width: f64) -> Arc<GlobalStateMap> {
    Arc::new(GlobalStateMap::from_iter([("width".to_owned(), Value::Double(width))]))
}

fn evaluate_with_state(
    property: &mut Transitioning<PropertyValue<f32>>,
    global_state: Arc<GlobalStateMap>,
    delta: Duration,
) -> f32 {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let mut parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);
    parameters.global_state = Some(global_state);

    *property.evaluate(Evaluated::plain, &parameters, 0.0_f32).uniform()
}

fn global_state_change(
    prior_state: Arc<GlobalStateMap>,
    changed_key: &str,
    delta: Duration,
    state_only: bool,
) -> TransitionParameters {
    let transition = TransitionOptions { duration: Some(Duration::from_millis(1000)), ..TransitionOptions::default() };
    TransitionParameters {
        now: Some(start() + delta),
        transition,
        global_state_change: Some(GlobalStateChange {
            prior_state,
            changed_keys: Arc::new(VecSet::from_iter([changed_key.to_owned()])),
            state_only,
        }),
    }
}

fn untransitioned() -> TransitionParameters {
    TransitionParameters::new(Some(start()), TransitionOptions::default())
}

fn width_transitionable() -> Transitionable<PropertyValue<f32>> {
    Transitionable { value: global_state_value("width"), options: TransitionOptions::default() }
}

// gtest: TransitioningPropertyValue.GlobalStateChangeTransitionsFromPriorState
#[test]
fn global_state_change_transitions_from_prior_state() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());
    assert_float_eq(0.0, evaluate_with_state(&mut property, width_state(0.0), Duration::from_millis(0)));

    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);

    assert_float_eq(0.0, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(1500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeContinuesRunningTransition
#[test]
fn global_state_change_continues_running_transition() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());
    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);
    assert_float_eq(0.823_099, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(500)));

    // Changing the state back halfway starts from the value shown, not from the state now read.
    property =
        width.transition(&global_state_change(width_state(1.0), "width", Duration::from_millis(500), true), property);

    assert_float_eq(0.823_099, evaluate_with_state(&mut property, width_state(0.0), Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate_with_state(&mut property, width_state(0.0), Duration::from_millis(2500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeWithLayerChangeTransitionsFromPriorState
#[test]
fn global_state_change_with_layer_change_transitions_from_prior_state() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());
    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);
    assert_float_eq(0.823_099, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(500)));

    // A layer change in the same frame as the state change restarts the transition from the value shown.
    property =
        width.transition(&global_state_change(width_state(1.0), "width", Duration::from_millis(500), false), property);

    assert_float_eq(0.823_099, evaluate_with_state(&mut property, width_state(0.0), Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate_with_state(&mut property, width_state(0.0), Duration::from_millis(2500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeSnapsPatternImage
#[test]
fn global_state_change_snaps_pattern_image() {
    let json = parse_json(r#"["image", ["global-state", "pattern"]]"#);
    let mut error = Error::default();
    let pattern = convert_property_value::<Image>(
        &Convertible::from(&json),
        &mut error,
        true,
        false,
        &Functions::with_builtins(),
    )
    .unwrap_or_else(|| panic!("{}", error.message));
    let value = Transitionable { value: pattern, options: TransitionOptions::default() };
    let mut property = value.transition(&untransitioned(), Transitioning::default());

    // Pattern images are laid out into tiles, not interpolated, so the state change applies at once.
    let prior_state = Arc::new(GlobalStateMap::from_iter([("pattern".to_owned(), Value::String("a".to_owned()))]));
    property = value.transition(&global_state_change(prior_state, "pattern", Duration::from_millis(0), true), property);

    assert!(!property.has_transition());
}

// gtest: TransitioningPropertyValue.GlobalStateChangeOfUnreadKeyKeepsValue
#[test]
fn global_state_change_of_unread_key_keeps_value() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());

    property =
        width.transition(&global_state_change(width_state(0.0), "color", Duration::from_millis(0), true), property);

    assert!(!property.has_transition());
    assert_float_eq(1.0, evaluate_with_state(&mut property, width_state(1.0), Duration::from_millis(0)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeTransitionsLightProperty
#[test]
fn global_state_change_transitions_light_property() {
    let mut light = LightProperties::Transitionable::default();
    light.light_intensity.value = global_state_value("width");
    let mut transitioning = light.transitioned(&untransitioned(), light.untransitioned());

    // A state-only change transitions the property that reads the key and leaves the others alone.
    transitioning = light
        .transitioned(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), transitioning);

    assert!(transitioning.light_intensity.has_transition());
    assert!(!transitioning.light_color.has_transition());
    let intensity = &mut transitioning.light_intensity;
    assert_float_eq(0.0, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(1500)));
}

// A paint set cross-fades where one of its properties' kinds does.
const _: () = {
    use crate::layer_type::{NoProperties, PaintSet};
    use crate::tests::fixture_layer::{DotPaintProperties, PlainPaintProperties};

    assert!(<PlainPaintProperties::Transitionable as PaintSet>::CROSSFADES);
    assert!(!<DotPaintProperties::Transitionable as PaintSet>::CROSSFADES);
    assert!(!<NoProperties as PaintSet>::CROSSFADES);
};

/// `property` evaluated `delta` after the tests' start, at zoom `zoom`.
fn evaluate_at<T>(property: &mut Transitioning<PropertyValue<T>>, delta: Duration, zoom: f32) -> T
where
    PropertyValue<T>: crate::property_value::StyleValue<Type = T>,
    T: mln_util::interpolate::Interpolate + crate::expression::ValueConverter + Clone + Default,
{
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(zoom, Some(start() + delta));
    let parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);
    property.evaluate(Evaluated::plain, &parameters, T::default()).uniform().clone()
}

/// A value of `T` converted from `json`, as a property's style value.
fn convert<T: crate::conversion::Converter + crate::expression::ValueConverter + Clone>(
    json: &str,
) -> PropertyValue<T> {
    let mut error = Error::default();
    let doc = parse_json(json);
    convert_property_value(&Convertible::from(&doc), &mut error, false, false, &Functions::with_builtins())
        .unwrap_or_else(|| panic!("{}", error.message))
}

#[test]
fn a_position_is_a_longitude_and_a_latitude_that_follow_zoom_and_transition_in_a_line() {
    use crate::property_value::StyleValue as _;

    let position = convert::<mln_util::LatLng>("[-122.4, 37.8]");
    assert_eq!(Some(&mln_util::LatLng::new(37.8, -122.4).unwrap()), position.as_constant());
    let json = super::util::value_to_json(position.style_property().get_value());
    assert_eq!(parse_json("[-122.4, 37.8]"), json);

    let by_zoom = convert::<mln_util::LatLng>(
        r#"["interpolate", ["linear"], ["zoom"], 0, ["literal", [0, 0]], 10, ["literal", [20, 10]]]"#,
    );
    let mut by_zoom = Transitioning::new(by_zoom);
    assert_eq!(mln_util::LatLng::new(5.0, 10.0).unwrap(), evaluate_at(&mut by_zoom, Duration::ZERO, 5.0));

    let transition = TransitionOptions { duration: Some(Duration::from_millis(1000)), ..TransitionOptions::default() };
    let from = Transitioning::new(PropertyValue::Constant(mln_util::LatLng::new(10.0, 170.0).unwrap()));
    let to = PropertyValue::Constant(mln_util::LatLng::new(20.0, 190.0).unwrap());
    let mut moving = Transitioning::with_prior(to, from, &transition, Some(start()));
    let midway = evaluate_at(&mut moving, Duration::from_millis(500), 0.0);
    assert_float_eq(10.0 + 10.0 * 0.823_099, midway.latitude() as f32);
    assert_float_eq(170.0 + 20.0 * 0.823_099, midway.longitude() as f32);
}

#[test]
fn a_rotation_transitions_the_shorter_way_around() {
    use mln_style_types::Rotation;

    assert_eq!(Some(&Rotation::new(350.0)), convert::<Rotation>("-10").as_constant());

    let transition = TransitionOptions { duration: Some(Duration::from_millis(1000)), ..TransitionOptions::default() };
    let from = Transitioning::new(PropertyValue::Constant(Rotation::new(350.0)));
    let mut turning =
        Transitioning::with_prior(PropertyValue::Constant(Rotation::new(10.0)), from, &transition, Some(start()));
    let midway = evaluate_at(&mut turning, Duration::from_millis(500), 0.0).degrees();
    assert!((midway - (350.0 + 20.0 * 0.823_099 - 360.0)).abs() < 1e-4, "{midway}");
    assert_eq!(10.0, evaluate_at(&mut turning, Duration::from_millis(1000), 0.0).degrees());
}
