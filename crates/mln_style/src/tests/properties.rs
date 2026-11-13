use std::sync::{Arc, LazyLock};
use std::time::Duration;

use feature::Value;
use mln_conversion::{Convertible, Error};
use mln_util::time::Instant;
use mln_vec_map::VecSet;

use super::util::parse_json;
use crate::conversion::convert_property_value;
use crate::expression::{GlobalStateMap, Image, dsl};
use crate::light::{LightColor, LightIntensity, LightProperties};
use crate::map::zoom_history::ZoomHistory;
use crate::properties::{Transitionable, Transitioning};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;
use crate::renderer::data_driven_property_evaluator::DataDrivenPropertyEvaluator;
use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::PropertyEvaluator;
use crate::renderer::transition_parameters::{GlobalStateChange, TransitionParameters};
use crate::transition_options::TransitionOptions;

/// The time the tests' transitions start at.
fn start() -> Instant {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    *START
}

fn evaluate(property: &Transitioning<PropertyValue<f32>>, delta: Duration) -> f32 {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);

    let evaluator = PropertyEvaluator::new(&parameters, 0.0_f32);

    property.evaluate(&evaluator, parameters.now)
}

/// gtest `ASSERT_FLOAT_EQ`: within 4 ULPs.
fn assert_float_eq(expected: f32, actual: f32) {
    let diff = (i64::from(expected.to_bits()) - i64::from(actual.to_bits())).abs();
    assert!(diff <= 4, "expected {expected}, got {actual}");
}

// gtest: TransitioningPropertyValue.EvaluateDefaultValue
#[test]
fn evaluate_default_value() {
    let property = Transitioning::<PropertyValue<f32>>::default();
    assert_eq!(0.0_f32, evaluate(&property, Duration::ZERO));
}

// gtest: TransitioningPropertyValue.EvaluateUntransitionedConstant
#[test]
fn evaluate_untransitioned_constant() {
    let property = Transitioning::with_prior(
        PropertyValue::Constant(1.0_f32),
        Transitioning::default(),
        &TransitionOptions::default(),
        Some(start()),
    );

    assert_eq!(1.0_f32, evaluate(&property, Duration::ZERO));
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

    let t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));

    assert_float_eq(0.0, evaluate(&t1, Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate(&t1, Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate(&t1, Duration::from_millis(1500)));
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

    let t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));

    assert_float_eq(0.0, evaluate(&t1, Duration::from_millis(0)));
    assert_float_eq(0.0, evaluate(&t1, Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate(&t1, Duration::from_millis(612)));
    assert_float_eq(0.823_099, evaluate(&t1, Duration::from_millis(1500)));
    assert_float_eq(1.0, evaluate(&t1, Duration::from_millis(2500)));
}

fn evaluate_data_expression(
    property: &Transitioning<PropertyValue<f32>>,
    delta: Duration,
) -> PossiblyEvaluatedPropertyValue<f32> {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);

    let evaluator = DataDrivenPropertyEvaluator::<f32>::new(&parameters, 0.0_f32);

    property.evaluate(&evaluator, parameters.now)
}

// gtest: TransitioningDataDrivenPropertyValue.Evaluate
#[test]
fn transitioning_data_driven_property_value_evaluate() {
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

    let expression = PropertyExpression::<f32>::from_box(dsl::number(dsl::get_str("property_name"), None), None);

    let t1 = Transitioning::with_prior(PropertyValue::Expression(expression), t0.clone(), &transition, Some(start()));

    assert!(evaluate_data_expression(&t0, Duration::from_millis(0)).is_constant());
    assert!(
        !evaluate_data_expression(&t1, Duration::from_millis(0)).is_constant(),
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
    let t1 = Transitioning::with_prior(PropertyValue::Constant(1.0_f32), t0, &transition, Some(start()));
    assert!(t1.has_transition());
    assert_float_eq(0.823_099, evaluate(&t1, Duration::from_millis(500)));
    assert!(t1.has_transition());
    assert_eq!(1.0, evaluate(&t1, Duration::from_millis(1000)));
    assert!(!t1.has_transition());
}

/// `PossiblyEvaluated::evaluate::<P>(z, feature, ...)`, as the circle layer
/// and the line bucket call it.
#[test]
fn possibly_evaluated_evaluate_per_feature() {
    use mln_expression::dsl::{get_str, number};
    use mln_style_types::{AlignmentType, LineJoinType};
    use mln_tile_id::CanonicalTileID;

    use crate::layers::circle_layer_properties::{CirclePitchAlignment, CircleRadius};
    use crate::layers::line_layer_properties::LineJoin;
    use crate::layers::{CirclePaintProperties, LineLayoutProperties};
    use crate::property_expression::PropertyExpression;
    use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;
    use crate::tests::util::{StubGeometryTileFeature, property_map};

    let feature = StubGeometryTileFeature::new(property_map(&[("r", Value::Double(7.5))]));

    // A constant.
    let mut paint = CirclePaintProperties::PossiblyEvaluated {
        circle_radius: PossiblyEvaluatedPropertyValue::Constant(3.0),
        ..CirclePaintProperties::PossiblyEvaluated::default()
    };
    assert_eq!(3.0, paint.evaluate::<CircleRadius>(10.0, &feature));
    // A feature expression, with the property's default for a missing value.
    paint.circle_radius =
        PossiblyEvaluatedPropertyValue::Expression(PropertyExpression::from_box(number(get_str("r"), None), None));
    assert_eq!(7.5, paint.evaluate::<CircleRadius>(10.0, &feature));
    let other = StubGeometryTileFeature::new(property_map(&[]));
    assert_eq!(5.0, paint.evaluate::<CircleRadius>(10.0, &other));
    // A plain (non-data-driven) property.
    paint.circle_pitch_alignment = AlignmentType::Map;
    assert_eq!(AlignmentType::Map, paint.evaluate::<CirclePitchAlignment>(10.0, &feature));

    let layout = LineLayoutProperties::PossiblyEvaluated::default();
    assert_eq!(
        LineJoinType::Miter,
        layout.evaluate_canonical::<LineJoin>(10.0, &feature, &CanonicalTileID::new(0, 0, 0))
    );
}

fn global_state_value(key: &str) -> PropertyValue<f32> {
    let json = parse_json(&format!(r#"["global-state", "{key}"]"#));
    let mut error = Error::default();
    let value = convert_property_value::<f32>(&Convertible::from(&json), &mut error, false, false);
    value.unwrap_or_else(|| panic!("{}", error.message))
}

fn width_state(width: f64) -> Arc<GlobalStateMap> {
    Arc::new(GlobalStateMap::from_iter([("width".to_owned(), Value::Double(width))]))
}

fn evaluate_with_state(
    property: &Transitioning<PropertyValue<f32>>,
    global_state: Arc<GlobalStateMap>,
    delta: Duration,
) -> f32 {
    let mut zoom_history = ZoomHistory::default();
    zoom_history.update(0.0, Some(start() + delta));

    let mut parameters =
        PropertyEvaluationParameters::with_zoom_history(zoom_history, Some(start() + delta), Duration::ZERO);
    parameters.global_state = Some(global_state);

    let evaluator = PropertyEvaluator::new(&parameters, 0.0_f32);

    property.evaluate(&evaluator, parameters.now)
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
    assert_float_eq(0.0, evaluate_with_state(&property, width_state(0.0), Duration::from_millis(0)));

    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);

    assert_float_eq(0.0, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(1500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeContinuesRunningTransition
#[test]
fn global_state_change_continues_running_transition() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());
    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);
    assert_float_eq(0.823_099, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(500)));

    // Changing the state back halfway starts from the value shown, not from the state now read.
    property =
        width.transition(&global_state_change(width_state(1.0), "width", Duration::from_millis(500), true), property);

    assert_float_eq(0.823_099, evaluate_with_state(&property, width_state(0.0), Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate_with_state(&property, width_state(0.0), Duration::from_millis(2500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeWithLayerChangeTransitionsFromPriorState
#[test]
fn global_state_change_with_layer_change_transitions_from_prior_state() {
    let width = width_transitionable();
    let mut property = width.transition(&untransitioned(), Transitioning::default());
    property =
        width.transition(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), property);
    assert_float_eq(0.823_099, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(500)));

    // A layer change in the same frame as the state change restarts the transition from the value shown.
    property =
        width.transition(&global_state_change(width_state(1.0), "width", Duration::from_millis(500), false), property);

    assert_float_eq(0.823_099, evaluate_with_state(&property, width_state(0.0), Duration::from_millis(500)));
    assert_float_eq(0.0, evaluate_with_state(&property, width_state(0.0), Duration::from_millis(2500)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeSnapsPatternImage
#[test]
fn global_state_change_snaps_pattern_image() {
    let json = parse_json(r#"["image", ["global-state", "pattern"]]"#);
    let mut error = Error::default();
    let pattern = convert_property_value::<Image>(&Convertible::from(&json), &mut error, true, false)
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
    assert_float_eq(1.0, evaluate_with_state(&property, width_state(1.0), Duration::from_millis(0)));
}

// gtest: TransitioningPropertyValue.GlobalStateChangeTransitionsLightProperty
#[test]
fn global_state_change_transitions_light_property() {
    let mut light = LightProperties::Transitionable::default();
    light.get_mut::<LightIntensity>().value = global_state_value("width");
    let mut transitioning = light.transitioned(&untransitioned(), light.untransitioned());

    // A state-only change transitions the property that reads the key and leaves the others alone.
    transitioning = light
        .transitioned(&global_state_change(width_state(0.0), "width", Duration::from_millis(0), true), transitioning);

    assert!(transitioning.get::<LightIntensity>().has_transition());
    assert!(!transitioning.get::<LightColor>().has_transition());
    let intensity = transitioning.get::<LightIntensity>();
    assert_float_eq(0.0, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(0)));
    assert_float_eq(0.823_099, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(500)));
    assert_float_eq(1.0, evaluate_with_state(intensity, width_state(1.0), Duration::from_millis(1500)));
}
