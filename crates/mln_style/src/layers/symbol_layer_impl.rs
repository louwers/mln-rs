// The format section override checks and `update_overrides` are here;
// `set_overrides`, which builds `FormatSectionOverride` expressions, is in
// format_section_override.rs.

use mln_expression::format_expression::FormatExpressionSection;
use mln_expression::value::from_expression_value;

use crate::evaluated::Evaluated;
use crate::expression::{Expr, Formatted, FormattedSection, Type};
use crate::layer_type::LayerStyle;
use crate::layers::Symbol;
use crate::layers::symbol_layer_properties::SymbolPaintProperties;
use crate::properties::OverridableSection;
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;

/// The alternatives of a `PropertyValue<Formatted>` or an
/// `Evaluated<Formatted>` that `has_override` reads.
pub enum FormattedValue<'a> {
    Undefined,
    Constant(&'a Formatted),
    Expression(&'a PropertyExpression<Formatted>),
}

pub trait AsFormattedValue {
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
pub struct SymbolLayerPaintPropertyOverrides;

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
    pub fn has_overrides(formatted: &dyn AsFormattedValue) -> bool {
        let mut result = false;
        result |=
            Self::has_override(formatted, FormattedSection::has_text_color, FormatExpressionSection::has_text_color);
        result
    }

    /// Carries a `FormatSectionOverride` wrapper of `text-color` over from
    /// `evaluated` to `updated`.
    #[cfg(test)]
    // gtests-data: ported for Layer.SymbolLayerOverrides.
    pub(crate) fn update_overrides(
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
        updated: &mut SymbolPaintProperties::PossiblyEvaluated,
    ) {
        let property = evaluated.text_color.clone();
        if !property.is_constant() {
            let has_format_section_override =
                property.expression().is_some_and(|e| e.get_expression().as_format_section_override().is_some());
            if has_format_section_override {
                updated.text_color = property;
            }
        }
    }

    /// Whether a constant overridable property differs between `lhs` and
    /// `rhs`.
    #[must_use]
    pub(crate) fn has_paint_property_difference(
        lhs: &SymbolPaintProperties::Transitionable,
        rhs: &SymbolPaintProperties::Transitionable,
    ) -> bool {
        let mut result = false;
        let (l, r) = (&lhs.text_color.value, &rhs.text_color.value);
        result |= l.is_constant() && r.is_constant() && (l.as_constant() != r.as_constant());
        result
    }
}

/// The style data of a `symbol` layer.
pub type Impl = LayerStyle<Symbol>;
