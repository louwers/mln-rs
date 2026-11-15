// The format section override checks and `update_overrides` are here;
// `set_overrides`, which builds `FormatSectionOverride` expressions, is in
// format_section_override.rs.

use std::sync::OnceLock;

use mln_expression::format_expression::FormatExpressionSection;
use mln_expression::value::from_expression_value;
use mln_util::constants::{LAST_RESORT_ALPHABETIC_FONT, LAST_RESORT_PAN_UNICODE_FONT};
use mln_util::logging::{Event, Log};
use mln_vec_map::VecSet;

use crate::evaluated::Evaluated;
use crate::expression::{Dependency, Expr, Formatted, FormattedSection, Type};
use crate::layer::{self, FontStack, ImplBase, LayerTypeInfo};
use crate::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties};
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

#[derive(Clone, Debug, mln_wire::Wire)]
pub struct Impl {
    pub(crate) base: ImplBase,
    pub layout: SymbolLayoutProperties::Unevaluated,
    pub paint: SymbolPaintProperties::Transitionable,
    #[wire(skip)]
    has_format_section_overrides_: OnceLock<bool>,
}

impl Impl {
    #[must_use]
    pub(crate) fn new(layer_id: String, source_id: String) -> Self {
        Impl {
            base: ImplBase::new(layer_id, source_id),
            layout: SymbolLayoutProperties::Unevaluated::default(),
            paint: SymbolPaintProperties::Transitionable::default(),
            has_format_section_overrides_: OnceLock::new(),
        }
    }

    fn has_format_section_overrides(&self) -> bool {
        *self
            .has_format_section_overrides_
            .get_or_init(|| SymbolLayerPaintPropertyOverrides::has_overrides(&self.layout.text_field))
    }
}

/// The base members read as `impl_.id`, `impl_.visibility`, and so on.
impl std::ops::Deref for Impl {
    type Target = ImplBase;
    fn deref(&self) -> &ImplBase {
        &self.base
    }
}

impl std::ops::DerefMut for Impl {
    fn deref_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }
}

impl layer::Impl for Impl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn has_layout_difference(&self, other: &dyn layer::Impl) -> bool {
        debug_assert!(std::ptr::eq(other.get_type_info(), layer::Impl::get_type_info(self)));
        let impl_ = other.downcast_ref::<Impl>().expect("SymbolLayer::Impl::hasLayoutDifference: not a symbol layer");
        self.filter != impl_.filter
            || self.visibility != impl_.visibility
            || self.layout != impl_.layout
            || self.paint.has_data_driven_property_difference(&impl_.paint)
            || (self.has_format_section_overrides()
                && SymbolLayerPaintPropertyOverrides::has_paint_property_difference(&self.paint, &impl_.paint))
    }

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn populate_font_stack(&self, font_stack: &mut VecSet<FontStack>) {
        if self.layout.text_field.is_undefined() {
            return;
        }

        match &self.layout.text_font {
            PropertyValue::Undefined => {
                font_stack
                    .insert(vec![LAST_RESORT_ALPHABETIC_FONT.to_owned(), LAST_RESORT_PAN_UNICODE_FONT.to_owned()]);
            }
            PropertyValue::Constant(constant) => {
                font_stack.insert(constant.clone());
            }
            PropertyValue::Expression(function) => {
                for value in function.possible_outputs() {
                    if let Some(value) = value {
                        font_stack.insert(value);
                    } else {
                        Log::warning(
                            Event::ParseStyle,
                            format!(
                                "Layer '{}' has an invalid value for text-font and will not render text. Output \
                                 values must be contained as literals within the expression.",
                                self.id
                            ),
                        );
                        break;
                    }
                }
            }
        }
    }

    fn get_layout_dependencies(&self) -> Dependency {
        self.layout.get_dependencies() | self.base.get_filter_dependencies()
    }

    fn collect_layout_global_state_refs(&self, refs: &mut VecSet<String>) {
        self.layout.collect_global_state_refs(refs);
        self.base.collect_filter_global_state_refs(refs);
    }
}
