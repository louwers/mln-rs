// The style tests of the symbol layer type's `FormatSectionOverride`:
// PropertyExpression.FormatSectionOverride,
// GlobalStateExpression.FormatSectionOverrideUsesCapturedState and
// Layer.SymbolLayerOverrides.

use std::borrow::Cow;
use std::sync::Arc;

use feature::{PropertyMap, Value as MbglValue};
use mln_conversion::Convertible;
use mln_expression::dsl::{format_str, get_str, literal_str, string, to_color};
use mln_expression::expression::{Dependency, EvaluationContext, Expr, GlobalStateMap};
use mln_expression::format_expression::{FormatExpression, FormatExpressionSection};
use mln_expression::formatted::{Formatted, FormattedSection};
use mln_expression::match_::Match;
use mln_expression::parsing_context::ParsingContext;
use mln_expression::type_::Type;
use mln_expression::value::{ObjectType, Value};
use mln_style::evaluated::Evaluated;
use mln_style::expression::Functions;
use mln_style::layer_type::{ErasedLayerStyle as _, LayerStyle};
use mln_style::properties::{Transitionable, Transitioning};
use mln_style::property_expression::PropertyExpression;
use mln_style::property_value::PropertyValue;
use mln_style::transition_options::TransitionOptions;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::Color;
use mln_util::geometry::FeatureType;
use mln_vec_map::{VecMap, VecSet};

use crate::symbol::format_section_override::{
    SymbolLayerPaintPropertyOverrides as MockOverrides, format_section_override as new_override, set_overrides,
};
use crate::symbol::{Symbol, SymbolLayoutProperties, SymbolPaintProperties};

/// `test/include/mln/test/stub_geometry_tile_feature.hpp` (the property-map
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

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&self.properties)
    }
}

/// A value that differs by feature, with the type's default.
fn per_feature<T: Default>(expression: PropertyExpression<T>) -> Evaluated<T> {
    Evaluated::PerFeature { expression, default: T::default() }
}

fn create_override(prop_value: Evaluated<Color>, prop_name: &str) -> Arc<Expr> {
    Arc::new(new_override(prop_value, prop_name.to_owned()))
}

/// Parses an expression from JSON text, with an optional expected type.
fn parse_expression(json: &str, expected: Option<Type>) -> Option<Expr> {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut ctx = match expected {
        Some(t) => ParsingContext::with_expected(&Functions::with_builtins(), t),
        None => ParsingContext::new(&Functions::with_builtins()),
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
    let constant_value_red = Evaluated::Constant(Color::red());
    let constant_value_green = Evaluated::Constant(Color::green());
    let dds_value_red =
        per_feature(PropertyExpression::<Color>::from_expr(to_color(string(get_str("color"), None), None)));

    assert_eq!(Dependency::FEATURE, dds_value_red.dependencies());

    // Evaluation test
    {
        let override1 = create_override(constant_value_green.clone(), "text-color");
        let prop_expr = PropertyExpression::<Color>::new(override1);
        assert_eq!(
            Color::green(),
            prop_expr.evaluate(&EvaluationContext::with_zoom_feature(15.0, Some(&one_double)), Color::default())
        );
        assert_eq!(
            Color::green(),
            prop_expr.evaluate(&EvaluationContext::from_feature(Some(&one_double)), Color::default())
        );
        assert_eq!(Color::blue(), prop_expr.evaluate(&ctx, Color::default()));

        // Override implies a feature dependency (See `::isFeatureConstant`)
        assert_eq!(Dependency::OVERRIDE | Dependency::FEATURE, prop_expr.get_dependencies());

        let override2 = create_override(dds_value_red.clone(), "text-color");
        let prop_expr_dds = PropertyExpression::<Color>::new(override2);
        assert_eq!(
            Color::red(),
            prop_expr_dds.evaluate(&EvaluationContext::from_feature(Some(&one_color)), Color::default())
        );
        assert_eq!(Color::blue(), prop_expr_dds.evaluate(&ctx, Color::default()));
    }

    // Equality test
    {
        let override1 = create_override(constant_value_red, "text-color");
        let override2 = create_override(constant_value_green.clone(), "text-color");
        let override3 = create_override(constant_value_green, "text-halo-color");
        let override4 = create_override(dds_value_red, "text-color");
        assert_ne!(override1, override2);
        assert_ne!(override2, override3);
        assert_ne!(override1, override4);
    }
}

// gtest: GlobalStateExpression.FormatSectionOverrideUsesCapturedState
#[test]
fn format_section_override_uses_captured_state() {
    let expr = parse_expression(r#"["to-color", ["global-state", "labelColor"]]"#, Some(Type::Color)).unwrap();

    let mut property_expression = PropertyExpression::<Color>::from_expr(expr);
    let state: Arc<GlobalStateMap> =
        Arc::new(GlobalStateMap::from([("labelColor".to_owned(), MbglValue::String("red".to_owned()))]));
    property_expression.capture_global_state(Some(state));

    let default_value = per_feature(property_expression);
    let override_expression = new_override(default_value, "text-color".to_owned());

    // Evaluating without a formatted section and without global state in the
    // context must fall back to the state captured by the wrapped expression.
    let result = override_expression.evaluate(&EvaluationContext::default());
    assert_eq!(Ok(Value::Color(Color::parse("red").unwrap())), result.map_err(|e| e.message().to_owned()));
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
        let expression: Expr = match self {
            // Expression, no overrides.
            FormattedCase::ExpressionNoOverride => FormatExpression::new_expr(Vec::new()),
            // Expression, overridden text-color.
            FormattedCase::ExpressionOverride => {
                let mut section = FormatExpressionSection::new(literal_str(""));
                section.set_text_section_options(None, None, Some(to_color(literal_str("red"), None)));
                FormatExpression::new_expr(vec![section])
            }
            // Nested expressions, overridden text-color.
            FormattedCase::NestedOverride => {
                let formatted_expr1 = format_str("first paragraph");
                let mut second_paragraph = FormatExpressionSection::new(literal_str("second paragraph"));
                second_paragraph.set_text_section_options(None, None, Some(to_color(literal_str("blue"), None)));
                let formatted_expr2: Expr = FormatExpression::new_expr(vec![second_paragraph]);
                let mut branches: VecMap<String, Expr> = VecMap::new();
                branches.insert("1st".to_owned(), formatted_expr1);
                branches.insert("2nd".to_owned(), formatted_expr2);
                Match::new_expr(Type::Formatted, literal_str("input"), branches, format_str("otherwise"))
            }
            _ => return None,
        };
        Some(PropertyExpression::from_expr(expression))
    }
}

/// `layout` (a `PossiblyEvaluated` set in C++) as the `Unevaluated` layout
/// the port's `set_overrides` takes: only its text-field is read, through
/// `hasOverrides`, which gives the same answer for both.
fn unevaluated_layout_with_text_field(formatted: Formatted) -> SymbolLayoutProperties::Unevaluated {
    SymbolLayoutProperties::Unevaluated {
        text_field: PropertyValue::Constant(formatted),
        ..SymbolLayoutProperties::Unevaluated::default()
    }
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
            layout.text_field = value;
            assert_eq!(case.has_override(), MockOverrides::has_overrides(&layout.text_field), "{case:?}");
        }

        // A constant overridable property's change counts while a section
        // overrides it.
        let color =
            |color| Transitionable { value: PropertyValue::Constant(color), options: TransitionOptions::default() };
        let mut current = LayerStyle::<Symbol>::new(String::new(), String::new());
        current.layout.text_field = PropertyValue::Constant(overridden_formatted());
        current.paint.text_color = color(Color::green());
        let mut updated = current.clone();
        assert!(!current.layout_difference(&updated));

        current.paint.text_color = color(Color::red());
        assert!(current.layout_difference(&updated));
        current.layout.text_field = PropertyValue::Undefined;
        updated.layout.text_field = PropertyValue::Undefined;
        assert!(!current.layout_difference(&updated));
    }

    // Possibly evaluated.
    {
        let mut layout = SymbolLayoutProperties::PossiblyEvaluated::default();
        let mut paint = SymbolPaintProperties::PossiblyEvaluated::default();
        // testHasOverrides<Evaluated>(layout); C++'s
        // `Evaluated<Formatted>()` is a default-constructed
        // (empty) constant, so its "Undefined" step has no override.
        for case in FORMATTED_CASES {
            let value = match case {
                FormattedCase::Undefined => Evaluated::default(),
                _ => case.constant().map(Evaluated::Constant).or_else(|| case.expression().map(per_feature)).unwrap(),
            };
            layout.text_field = value;
            assert_eq!(case.has_override(), MockOverrides::has_overrides(&layout.text_field), "{case:?}");
        }

        // Constant, overridden text-color.
        layout.text_field = Evaluated::Constant(overridden_formatted());
        paint.text_color = Evaluated::Constant(Color::red());
        assert!(paint.text_color.is_constant());
        set_overrides(
            &unevaluated_layout_with_text_field(overridden_formatted()),
            &mut paint,
            &SymbolPaintProperties::Unevaluated::default(),
            None,
        );
        assert!(!paint.text_color.is_constant());

        let mut updated = SymbolPaintProperties::PossiblyEvaluated {
            text_color: Evaluated::Constant(Color::red()),
            ..SymbolPaintProperties::PossiblyEvaluated::default()
        };
        MockOverrides::update_overrides(&paint, &mut updated);
        assert!(!updated.text_color.is_constant());
    }

    // A text-color folded to a constant from a global-state-only expression
    // keeps the dependency, its keys, and the state through the override
    // wrapper.
    {
        let mut paint = SymbolPaintProperties::PossiblyEvaluated {
            text_color: Evaluated::Constant(Color::red()),
            ..SymbolPaintProperties::PossiblyEvaluated::default()
        };

        let mut ctx = ParsingContext::new(&Functions::with_builtins());
        let state_expression = mln_expression::compound_expression::create_compound_expression(
            "global-state",
            vec![literal_str("labelColor")],
            &mut ctx,
        );
        let state_expression = state_expression.expect("global-state");
        let unevaluated = SymbolPaintProperties::Unevaluated {
            text_color: Transitioning::new(PropertyValue::Expression(PropertyExpression::<Color>::from_expr(
                to_color(state_expression, None),
            ))),
            ..SymbolPaintProperties::Unevaluated::default()
        };

        let state: Arc<GlobalStateMap> =
            Arc::new(GlobalStateMap::from([("labelColor".to_owned(), MbglValue::String("red".to_owned()))]));
        set_overrides(
            &unevaluated_layout_with_text_field(overridden_formatted()),
            &mut paint,
            &unevaluated,
            Some(&state),
        );

        let wrapped = &paint.text_color;
        assert!(!wrapped.is_constant());
        assert!(wrapped.dependencies().intersects(Dependency::GLOBAL_STATE));
        assert_eq!(Some(&VecSet::from(["labelColor".to_owned()])), wrapped.global_state_refs());

        // A section without an override evaluates from the captured state.
        let feature = StubGeometryTileFeature::new(&[]);
        let evaluated = match wrapped {
            Evaluated::PerFeature { expression: f, .. } => {
                f.evaluate(&EvaluationContext::with_zoom_feature(0.0, Some(&feature)), Color::black())
            }
            Evaluated::Constant(constant) => *constant,
            Evaluated::CrossFaded(faded) => faded.to,
        };
        assert_eq!(Color::parse("red").unwrap(), evaluated);
    }
}
