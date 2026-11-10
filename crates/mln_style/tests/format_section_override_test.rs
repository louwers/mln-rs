// Ports the tests of test/style/** that need `FormatSectionOverride`:
// PropertyExpression.
// FormatSectionOverride (test/style/property_expression.test.cpp),
// GlobalStateExpression.FormatSectionOverrideUsesCapturedState
// (test/style/expression/global_state.test.cpp) and Layer.SymbolLayerOverrides
// (test/style/style_layer.test.cpp).

use std::sync::Arc;

use feature::{PropertyMap, Value as MbglValue};
use mln_conversion::Convertible;
use mln_expression::dsl::{format_str, get_str, literal_str, string, to_color};
use mln_expression::expression::{Dependency, EvaluationContext, Expression, GlobalStateMap};
use mln_expression::format_expression::{FormatExpression, FormatExpressionSection};
use mln_expression::formatted::{Formatted, FormattedSection};
use mln_expression::match_::Match;
use mln_expression::parsing_context::ParsingContext;
use mln_expression::type_::Type;
use mln_expression::value::{ObjectType, Value};
use mln_style::format_section_override::{FormatSectionOverride, set_overrides};
use mln_style::layers::symbol_layer_impl::SymbolLayerPaintPropertyOverrides as MockOverrides;
use mln_style::layers::symbol_layer_properties::{TextColor, TextField};
use mln_style::layers::{SymbolLayoutProperties, SymbolPaintProperties};
use mln_style::properties::{Transitionable, Transitioning};
use mln_style::property_expression::PropertyExpression;
use mln_style::property_value::PropertyValue;
use mln_style::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;
use mln_style::transition_options::TransitionOptions;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::Color;
use mln_util::geometry::FeatureType;
use mln_vec_map::{VecMap, VecSet};

/// test/include/mln/test/stub_geometry_tile_feature.hpp (the property-map
/// constructor).
struct StubGeometryTileFeature {
    properties: PropertyMap,
}

impl StubGeometryTileFeature {
    fn new(entries: &[(&str, MbglValue)]) -> Self {
        StubGeometryTileFeature { properties: entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect() }
    }
}

impl GeometryTileFeature for StubGeometryTileFeature {
    fn get_type(&self) -> FeatureType {
        FeatureType::Point
    }

    fn get_value(&self, key: &str) -> Option<MbglValue> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> &PropertyMap {
        &self.properties
    }
}

/// `createOverride(exprType, propValue, propName)`.
fn create_override(
    expr_type: Type,
    prop_value: PossiblyEvaluatedPropertyValue<Color>,
    prop_name: &str,
) -> Arc<FormatSectionOverride> {
    Arc::new(FormatSectionOverride::new(expr_type, prop_value, prop_name.to_owned()))
}

/// `parseExpression(json, expected)` of global_state.test.cpp.
fn parse_expression(json: &str, expected: Option<Type>) -> Option<Box<dyn Expression>> {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut ctx = match expected {
        Some(t) => ParsingContext::with_expected(t),
        None => ParsingContext::new(),
    };
    ctx.parse_expression(&Convertible::from(&document), None)
}

// gtest: PropertyExpression.FormatSectionOverride
#[test]
fn format_section_override() {
    let one_double = StubGeometryTileFeature::new(&[("property", MbglValue::Double(1.0))]);
    let one_color = StubGeometryTileFeature::new(&[("color", MbglValue::String("red".to_owned()))]);

    let formatted_section = Value::Object(ObjectType::from([("text-color".to_owned(), Value::Color(Color::blue()))]));
    let ctx = EvaluationContext::from_feature(Some(&one_double)).with_formatted_section(Some(&formatted_section));
    let constant_value_red = PossiblyEvaluatedPropertyValue::Constant(Color::red());
    let constant_value_green = PossiblyEvaluatedPropertyValue::Constant(Color::green());
    let dds_value_red = PossiblyEvaluatedPropertyValue::Expression(PropertyExpression::<Color>::from_box(
        to_color(string(get_str("color"), None), None),
        None,
    ));

    assert_eq!(Dependency::FEATURE, dds_value_red.get_dependencies());

    // Evaluation test
    {
        let override1 = create_override(Type::Color, constant_value_green.clone(), "text-color");
        let prop_expr = PropertyExpression::<Color>::new(override1, None);
        assert_eq!(Color::green(), prop_expr.evaluate_zoom_feature(15.0, &one_double, Color::default()));
        assert_eq!(Color::green(), prop_expr.evaluate_feature(&one_double, Color::default()));
        assert_eq!(Color::blue(), prop_expr.evaluate(&ctx, Color::default()));

        // Override implies a feature dependency (See `::isFeatureConstant`)
        assert_eq!(Dependency::OVERRIDE | Dependency::FEATURE, prop_expr.get_dependencies());

        let override2 = create_override(Type::Color, dds_value_red.clone(), "text-color");
        let prop_expr_dds = PropertyExpression::<Color>::new(override2, None);
        assert_eq!(Color::red(), prop_expr_dds.evaluate_feature(&one_color, Color::default()));
        assert_eq!(Color::blue(), prop_expr_dds.evaluate(&ctx, Color::default()));
    }

    // Equality test
    {
        let override1 = create_override(Type::Color, constant_value_red, "text-color");
        let override2 = create_override(Type::Color, constant_value_green.clone(), "text-color");
        let override3 = create_override(Type::Color, constant_value_green, "text-halo-color");
        let override4 = create_override(Type::Color, dds_value_red, "text-color");
        assert!(!override1.equals(override2.as_ref()));
        assert!(!override2.equals(override3.as_ref()));
        assert!(!override1.equals(override4.as_ref()));
    }
}

// gtest: GlobalStateExpression.FormatSectionOverrideUsesCapturedState
#[test]
fn format_section_override_uses_captured_state() {
    let expr = parse_expression(r#"["to-color", ["global-state", "labelColor"]]"#, Some(Type::Color)).unwrap();

    let mut property_expression = PropertyExpression::<Color>::from_box(expr, None);
    let state: Arc<GlobalStateMap> =
        Arc::new(GlobalStateMap::from([("labelColor".to_owned(), MbglValue::String("red".to_owned()))]));
    property_expression.capture_global_state(Some(state));

    let default_value = PossiblyEvaluatedPropertyValue::Expression(property_expression);
    let override_expression = FormatSectionOverride::new(Type::Color, default_value, "text-color".to_owned());

    // Evaluating without a formatted section and without global state in the
    // context must fall back to the state captured by the wrapped expression.
    let result = override_expression.evaluate(&EvaluationContext::default());
    assert_eq!(Ok(Value::Color(Color::parse("red").unwrap())), result.map_err(|e| e.message));
}

/// The text-field values `testHasOverrides` sets, in its order.
#[derive(Clone, Copy, Debug)]
enum FormattedCase {
    Undefined,
    ConstantNoOverride,
    ConstantOverride,
    ExpressionNoOverride,
    ExpressionOverride,
    NestedOverride,
}

const FORMATTED_CASES: [FormattedCase; 6] = [
    FormattedCase::Undefined,
    FormattedCase::ConstantNoOverride,
    FormattedCase::ConstantOverride,
    FormattedCase::ExpressionNoOverride,
    FormattedCase::ExpressionOverride,
    FormattedCase::NestedOverride,
];

/// `Formatted("")` with a section that overrides text-color.
fn overridden_formatted() -> Formatted {
    let mut formatted = Formatted::from_plain("");
    formatted.sections.push(FormattedSection::new("section text".to_owned(), None, None, Some(Color::green())));
    formatted
}

impl FormattedCase {
    fn has_override(self) -> bool {
        matches!(
            self,
            FormattedCase::ConstantOverride | FormattedCase::ExpressionOverride | FormattedCase::NestedOverride
        )
    }

    fn constant(self) -> Option<Formatted> {
        match self {
            // Constant, no overrides.
            FormattedCase::ConstantNoOverride => Some(Formatted::from_plain("")),
            // Constant, overridden text-color.
            FormattedCase::ConstantOverride => Some(overridden_formatted()),
            _ => None,
        }
    }

    fn expression(self) -> Option<PropertyExpression<Formatted>> {
        let expression: Box<dyn Expression> = match self {
            // Expression, no overrides.
            FormattedCase::ExpressionNoOverride => Box::new(FormatExpression::new(Vec::new())),
            // Expression, overridden text-color.
            FormattedCase::ExpressionOverride => {
                let mut section = FormatExpressionSection::new(literal_str(""));
                section.set_text_section_options(None, None, Some(to_color(literal_str("red"), None)));
                Box::new(FormatExpression::new(vec![section]))
            }
            // Nested expressions, overridden text-color.
            FormattedCase::NestedOverride => {
                let formatted_expr1 = format_str("first paragraph");
                let mut second_paragraph = FormatExpressionSection::new(literal_str("second paragraph"));
                second_paragraph.set_text_section_options(None, None, Some(to_color(literal_str("blue"), None)));
                let formatted_expr2: Box<dyn Expression> = Box::new(FormatExpression::new(vec![second_paragraph]));
                let mut branches: VecMap<String, Arc<dyn Expression>> = VecMap::new();
                branches.insert("1st".to_owned(), Arc::from(formatted_expr1));
                branches.insert("2nd".to_owned(), Arc::from(formatted_expr2));
                Box::new(Match::<String>::new(Type::Formatted, literal_str("input"), branches, format_str("otherwise")))
            }
            _ => return None,
        };
        Some(PropertyExpression::from_box(expression, None))
    }
}

/// `layout` (a `PossiblyEvaluated` set in C++) as the `Unevaluated` layout
/// the port's `set_overrides` takes: only its text-field is read, through
/// `hasOverrides`, which gives the same answer for both.
fn unevaluated_layout_with_text_field(formatted: Formatted) -> SymbolLayoutProperties::Unevaluated {
    let mut layout = SymbolLayoutProperties::Unevaluated::default();
    *layout.get_mut::<TextField>() = PropertyValue::Constant(formatted);
    layout
}

// gtest: Layer.SymbolLayerOverrides
#[test]
fn symbol_layer_overrides() {
    // The C++ Mock*Properties are `Properties<TextField>` /
    // `Properties<TextColor>`; the symbol layer's sets hold the same tags.

    // Unevaluated / transitionable.
    {
        let mut layout = SymbolLayoutProperties::Unevaluated::default();
        // testHasOverrides<PropertyValue>(layout)
        for case in FORMATTED_CASES {
            let value = match case {
                FormattedCase::Undefined => PropertyValue::Undefined,
                _ => case
                    .constant()
                    .map(PropertyValue::Constant)
                    .or_else(|| case.expression().map(PropertyValue::Expression))
                    .unwrap(),
            };
            *layout.get_mut::<TextField>() = value;
            assert_eq!(case.has_override(), MockOverrides::has_overrides(layout.get::<TextField>()), "{case:?}");
        }

        let mut current = SymbolPaintProperties::Transitionable::default();
        let mut updated = SymbolPaintProperties::Transitionable::default();
        *current.get_mut::<TextColor>() =
            Transitionable { value: PropertyValue::Constant(Color::green()), options: TransitionOptions::default() };
        *updated.get_mut::<TextColor>() =
            Transitionable { value: PropertyValue::Constant(Color::green()), options: TransitionOptions::default() };
        assert!(!MockOverrides::has_paint_property_difference(&current, &updated));

        *current.get_mut::<TextColor>() =
            Transitionable { value: PropertyValue::Constant(Color::red()), options: TransitionOptions::default() };
        assert!(MockOverrides::has_paint_property_difference(&current, &updated));
    }

    // Possibly evaluated.
    {
        let mut layout = SymbolLayoutProperties::PossiblyEvaluated::default();
        let mut paint = SymbolPaintProperties::PossiblyEvaluated::default();
        // testHasOverrides<PossiblyEvaluatedPropertyValue>(layout); C++'s
        // `PossiblyEvaluatedPropertyValue<Formatted>()` is a default-constructed
        // (empty) constant, so its "Undefined" step has no override.
        for case in FORMATTED_CASES {
            let value = match case {
                FormattedCase::Undefined => PossiblyEvaluatedPropertyValue::default(),
                _ => case
                    .constant()
                    .map(PossiblyEvaluatedPropertyValue::Constant)
                    .or_else(|| case.expression().map(PossiblyEvaluatedPropertyValue::Expression))
                    .unwrap(),
            };
            *layout.get_mut::<TextField>() = value;
            assert_eq!(case.has_override(), MockOverrides::has_overrides(layout.get::<TextField>()), "{case:?}");
        }

        // Constant, overridden text-color.
        *layout.get_mut::<TextField>() = PossiblyEvaluatedPropertyValue::Constant(overridden_formatted());
        *paint.get_mut::<TextColor>() = PossiblyEvaluatedPropertyValue::Constant(Color::red());
        assert!(paint.get::<TextColor>().is_constant());
        set_overrides(
            &unevaluated_layout_with_text_field(overridden_formatted()),
            &mut paint,
            &SymbolPaintProperties::Unevaluated::default(),
            &None,
        );
        assert!(!paint.get::<TextColor>().is_constant());

        let mut updated = SymbolPaintProperties::PossiblyEvaluated::default();
        *updated.get_mut::<TextColor>() = PossiblyEvaluatedPropertyValue::Constant(Color::red());
        MockOverrides::update_overrides(&paint, &mut updated);
        assert!(!updated.get::<TextColor>().is_constant());
    }

    // A text-color folded to a constant from a global-state-only expression
    // keeps the dependency, its keys, and the state through the override
    // wrapper.
    {
        let mut paint = SymbolPaintProperties::PossiblyEvaluated::default();
        *paint.get_mut::<TextColor>() = PossiblyEvaluatedPropertyValue::Constant(Color::red());

        let mut ctx = ParsingContext::new();
        let state_expression = mln_expression::compound_expression::create_compound_expression(
            "global-state",
            vec![literal_str("labelColor")],
            &mut ctx,
        );
        let state_expression = state_expression.expect("global-state");
        let mut unevaluated = SymbolPaintProperties::Unevaluated::default();
        *unevaluated.get_mut::<TextColor>() = Transitioning::new(PropertyValue::Expression(
            PropertyExpression::<Color>::from_box(to_color(state_expression, None), None),
        ));

        let state: Arc<GlobalStateMap> =
            Arc::new(GlobalStateMap::from([("labelColor".to_owned(), MbglValue::String("red".to_owned()))]));
        set_overrides(
            &unevaluated_layout_with_text_field(overridden_formatted()),
            &mut paint,
            &unevaluated,
            &Some(state),
        );

        let wrapped = paint.get::<TextColor>();
        assert!(!wrapped.is_constant());
        assert!(wrapped.get_dependencies().intersects(Dependency::GLOBAL_STATE));
        assert_eq!(Some(&VecSet::from(["labelColor".to_owned()])), wrapped.get_global_state_refs());

        // A section without an override evaluates from the captured state.
        let feature = StubGeometryTileFeature::new(&[]);
        let evaluated = match wrapped {
            PossiblyEvaluatedPropertyValue::Expression(f) => {
                f.evaluate(&EvaluationContext::with_zoom_feature(0.0, Some(&feature)), Color::black())
            }
            PossiblyEvaluatedPropertyValue::Constant(constant) => *constant,
        };
        assert_eq!(Color::parse("red").unwrap(), evaluated);
    }
}
