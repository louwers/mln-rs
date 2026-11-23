//! The format sections of a symbol layer's `text-field` that override its
//! paint properties, and the `format-section-override` expressions that
//! give the overridden properties their sections' values. The only
//! overridable property is `text-color`.

use std::sync::Arc;

use mln_style::evaluated::Evaluated;
use mln_style::expression::format_expression::FormatExpressionSection;
use mln_style::expression::format_section_override::{FormatSectionOverride, OverrideDefault};
use mln_style::expression::value::from_expression_value;
use mln_style::expression::{Dependency, Expr, Formatted, FormattedSection, GlobalStateMap, Type};
use mln_style::properties::OverridableSection as _;
use mln_style::property_expression::PropertyExpression;
use mln_style::property_value::{PropertyValue, StyleValue as _};

use super::properties::{layout, paint};

/// The alternatives of a `PropertyValue<Formatted>` or an
/// `Evaluated<Formatted>` that `has_override` reads.
pub(crate) enum FormattedValue<'a> {
    Undefined,
    Constant(&'a Formatted),
    Expression(&'a PropertyExpression<Formatted>),
}

pub(crate) trait AsFormattedValue {
    fn as_formatted_value(&self) -> FormattedValue<'_>;
}

impl AsFormattedValue for PropertyValue<Formatted> {
    fn as_formatted_value(&self) -> FormattedValue<'_> {
        match self {
            PropertyValue::Undefined => FormattedValue::Undefined,
            PropertyValue::Constant(c) => FormattedValue::Constant(c),
            PropertyValue::Expression(e) => FormattedValue::Expression(e),
        }
    }
}

impl AsFormattedValue for Evaluated<Formatted> {
    fn as_formatted_value(&self) -> FormattedValue<'_> {
        match self {
            Evaluated::Constant(c) => FormattedValue::Constant(c),
            Evaluated::CrossFaded(faded) => FormattedValue::Constant(&faded.to),
            Evaluated::PerFeature { expression, .. } => FormattedValue::Expression(expression),
        }
    }
}

/// The format section overrides of the symbol paint properties. The only
/// overridable property is `text-color`, so the checks are written out for it
/// rather than folded over a list of overridable properties.
pub(crate) struct SymbolLayerPaintPropertyOverrides;

impl SymbolLayerPaintPropertyOverrides {
    /// Whether a section of `formatted` overrides a property, as the property's
    /// `has_override` for each section type tells.
    fn has_override(
        formatted: &dyn AsFormattedValue,
        section_has_override: fn(&FormattedSection) -> bool,
        expression_section_has_override: fn(&FormatExpressionSection) -> bool,
    ) -> bool {
        let check_literal = |literal: &Formatted| -> bool {
            for section in &literal.sections {
                if section_has_override(section) {
                    return true;
                }
            }
            false
        };

        match formatted.as_formatted_value() {
            FormattedValue::Constant(literal) => check_literal(literal),
            FormattedValue::Expression(property) => {
                fn check_expression<'a>(
                    e: &'a Expr,
                    found: &mut bool,
                    check_literal: &dyn Fn(&Formatted) -> bool,
                    expression_section_has_override: fn(&FormatExpressionSection) -> bool,
                ) {
                    if *found {
                        return;
                    }

                    if let Some(literal) = e.literal_value()
                        && e.get_type() == Type::Formatted
                    {
                        let formatted_value = from_expression_value::<Formatted>(literal);
                        if let Some(formatted_value) = formatted_value
                            && check_literal(&formatted_value)
                        {
                            *found = true;
                        }
                    } else if let Some(format_expr) = e.as_format() {
                        for section in format_expr.get_sections() {
                            if expression_section_has_override(section) {
                                *found = true;
                                break;
                            }
                        }
                    } else {
                        e.each_child(&mut |child| {
                            check_expression(child, found, check_literal, expression_section_has_override);
                        });
                    }
                }

                let mut expression_has_overrides = false;
                check_expression(
                    property.get_expression(),
                    &mut expression_has_overrides,
                    &check_literal,
                    expression_section_has_override,
                );
                expression_has_overrides
            }
            FormattedValue::Undefined => false,
        }
    }

    /// Whether `formatted` overrides any overridable property.
    pub(crate) fn has_overrides(formatted: &dyn AsFormattedValue) -> bool {
        let mut result = false;
        result |=
            Self::has_override(formatted, FormattedSection::has_text_color, FormatExpressionSection::has_text_color);
        result
    }

    /// Carries a `FormatSectionOverride` wrapper of `text-color` over from
    /// `evaluated` to `updated`.
    #[cfg(test)]
    // gtests-data: ported for Layer.SymbolLayerOverrides.
    pub(crate) fn update_overrides(evaluated: &paint::PossiblyEvaluated, updated: &mut paint::PossiblyEvaluated) {
        let property = evaluated.text_color.clone();
        if !property.is_constant() {
            let has_format_section_override =
                property.expression().is_some_and(|e| e.get_expression().as_format_section_override().is_some());
            if has_format_section_override {
                updated.text_color = property;
            }
        }
    }
}

/// A `format-section-override` of the color property `property_name`, whose
/// value is `default_value` where no section overrides it.
pub(crate) fn format_section_override(default_value: Evaluated<mln_util::Color>, property_name: String) -> Expr {
    let default_value = match default_value {
        Evaluated::Constant(color) => OverrideDefault::Constant(color),
        Evaluated::CrossFaded(faded) => OverrideDefault::Constant(faded.to),
        Evaluated::PerFeature { expression, .. } => OverrideDefault::Expression {
            expression: Arc::clone(expression.shared_expression()),
            captured_global_state: expression.get_captured_global_state().cloned(),
        },
    };
    FormatSectionOverride::new_expr(default_value, property_name)
}

/// Wraps `overridable`'s `text-color` in a `format-section-override`, for a
/// layer whose `text-field` in `overrides` has sections that override it:
/// each such section draws in its own color, and the rest in the
/// property's. A property folded to a constant from `unevaluated`'s
/// expression of the style's global state keeps reading `global_state`.
pub(crate) fn set_overrides(
    overrides: &layout::Unevaluated,
    overridable: &mut paint::PossiblyEvaluated,
    unevaluated: &paint::Unevaluated,
    global_state: Option<&Arc<GlobalStateMap>>,
) {
    if !SymbolLayerPaintPropertyOverrides::has_overrides(&overrides.text_field) {
        return;
    }
    let value = &mut overridable.text_color;
    // Wrapping a folded constant would drop the GlobalState dependency from
    // the wrapper and exempt the bucket from relayout tracking.
    if value.is_constant() {
        let unevaluated_value = unevaluated.text_color.get_value();
        if let Some(expression) = unevaluated_value.as_expression()
            && unevaluated_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
        {
            let mut retained = expression.clone();
            retained.capture_global_state(global_state.cloned());
            *value = Evaluated::PerFeature { expression: retained, default: paint::Defaults::text_color() };
        }
    }
    let override_ = format_section_override(std::mem::take(value), "text-color".to_owned());
    *value = Evaluated::PerFeature {
        expression: PropertyExpression::new(Arc::new(override_)),
        default: paint::Defaults::text_color(),
    };
}
