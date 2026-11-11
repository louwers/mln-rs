// Ports src/mln/style/layers/symbol_layer_impl.hpp +
// src/mln/style/layers/symbol_layer_impl.cpp. `staticTypeInfo()` and
// `stringifyLayout` are generated in C++ (layer.cpp.ejs); the first is in
// symbol_layer.rs.
//
// `FormatSectionOverrides` is split: the parts the style uses
// (`hasOverrides`, `hasOverride`, `hasPaintPropertyDifference`) and
// `updateOverrides` are here; `setOverride(s)`, which builds
// `expression::FormatSectionOverride`s, is in mln_renderer
// (format_section_override.rs).

use std::sync::OnceLock;

use mln_expression::format_expression::{FormatExpression, FormatExpressionSection};
use mln_expression::value::from_expression_value;
use mln_util::constants::{LAST_RESORT_ALPHABETIC_FONT, LAST_RESORT_PAN_UNICODE_FONT};
use mln_util::logging::{Event, Log};
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, Expression, Formatted, FormattedSection, Kind, Literal, Type, downcast};
use crate::layer::{self, FontStack, ImplBase, LayerTypeInfo};
use crate::layers::symbol_layer_properties::{
    SymbolLayoutProperties, SymbolPaintProperties, TextColor, TextField, TextFont,
};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;
use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;

/// The alternatives `hasOverride`'s `formatted.match(...)` visits; C++
/// templates it over `PropertyValue<Formatted>` and
/// `PossiblyEvaluatedPropertyValue<Formatted>`.
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

impl AsFormattedValue for PossiblyEvaluatedPropertyValue<Formatted> {
    fn as_formatted_value(&self) -> FormattedValue<'_> {
        match self {
            PossiblyEvaluatedPropertyValue::Constant(c) => FormattedValue::Constant(c),
            PossiblyEvaluatedPropertyValue::Expression(e) => FormattedValue::Expression(e),
        }
    }
}

/// `FormatSectionOverrides<SymbolPaintProperties::OverridableProperties>`
/// (`SymbolLayerPaintPropertyOverrides`). The overridable properties are
/// the paint properties with `IsOverridable` (`text-color`); the generic
/// C++ fold over them is written out for that one property.
pub struct SymbolLayerPaintPropertyOverrides;

impl SymbolLayerPaintPropertyOverrides {
    /// `hasOverride<Property>(formatted)`, with `Property::hasOverride` for
    /// both section types.
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
                let mut expression_has_overrides = false;
                fn check_expression<'a>(
                    e: &'a dyn Expression,
                    found: &mut bool,
                    check_literal: &dyn Fn(&Formatted) -> bool,
                    expression_section_has_override: fn(&FormatExpressionSection) -> bool,
                ) {
                    if *found {
                        return;
                    }

                    if e.get_kind() == Kind::Literal && *e.get_type() == Type::Formatted {
                        let literal_expr = downcast::<Literal>(e).expect("Kind::Literal is a Literal");
                        let formatted_value = from_expression_value::<Formatted>(literal_expr.get_value());
                        if let Some(formatted_value) = formatted_value {
                            if check_literal(&formatted_value) {
                                *found = true;
                            }
                        }
                    } else if e.get_kind() == Kind::FormatExpression {
                        let format_expr =
                            downcast::<FormatExpression>(e).expect("Kind::FormatExpression is a FormatExpression");
                        for section in format_expr.get_sections() {
                            if expression_section_has_override(section) {
                                *found = true;
                                break;
                            }
                        }
                    } else {
                        e.each_child(&mut |child| {
                            check_expression(child, found, check_literal, expression_section_has_override)
                        });
                    }
                }

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

    /// `hasOverrides(formatted)`: any overridable property is overridden.
    pub fn has_overrides(formatted: &dyn AsFormattedValue) -> bool {
        let mut result = false;
        result |= Self::has_override(
            formatted,
            TextColor::has_override::<FormattedSection>,
            TextColor::has_override::<FormatExpressionSection>,
        );
        result
    }

    /// `updateOverrides(evaluated, updated)` (`updateOverride<TextColor>`):
    /// carries a `FormatSectionOverride` wrapper over to `updated`.
    // gtests-data: ported for Layer.SymbolLayerOverrides.
    pub fn update_overrides(
        evaluated: &SymbolPaintProperties::PossiblyEvaluated,
        updated: &mut SymbolPaintProperties::PossiblyEvaluated,
    ) {
        let property = evaluated.get::<TextColor>().clone();
        if !property.is_constant() {
            let has_format_section_override = match &property {
                PossiblyEvaluatedPropertyValue::Expression(e) => {
                    e.get_expression().get_kind() == Kind::FormatSectionOverride
                }
                PossiblyEvaluatedPropertyValue::Constant(_) => false,
            };
            if has_format_section_override {
                *updated.get_mut::<TextColor>() = property;
            }
        }
    }

    /// `hasPaintPropertyDifference(lhs, rhs)`: a constant overridable
    /// property differs.
    pub fn has_paint_property_difference(
        lhs: &SymbolPaintProperties::Transitionable,
        rhs: &SymbolPaintProperties::Transitionable,
    ) -> bool {
        let mut result = false;
        let (l, r) = (&lhs.get::<TextColor>().value, &rhs.get::<TextColor>().value);
        result |= l.is_constant() && r.is_constant() && (l.as_constant() != r.as_constant());
        result
    }
}

/// `SymbolLayer::Impl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impl {
    pub base: ImplBase,
    pub layout: SymbolLayoutProperties::Unevaluated,
    pub paint: SymbolPaintProperties::Transitionable,
    /// `mutable std::optional<bool> hasFormatSectionOverrides_`.
    #[serde(skip)]
    has_format_section_overrides_: OnceLock<bool>,
}

impl Impl {
    /// `using Layer::Impl::Impl;` (`Impl(std::string layerID, std::string sourceID)`).
    pub fn new(layer_id: String, source_id: String) -> Self {
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
            .get_or_init(|| SymbolLayerPaintPropertyOverrides::has_overrides(self.layout.get::<TextField>()))
    }
}

/// `SymbolLayer::Impl : public Layer::Impl`: the base members read as
/// `impl_.id`, `impl_.visibility`, ... as in C++.
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

    fn stringify_layout(&self, _writer: &mut String) {
        // Generated in C++ (layer.cpp.ejs): `layout.stringify(writer)`.
        // TODO(port): conversion/stringify.hpp; nothing in the C++ core
        // calls stringifyLayout.
    }

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        Impl::static_type_info()
    }

    fn populate_font_stack(&self, font_stack: &mut VecSet<FontStack>) {
        if self.layout.get::<TextField>().is_undefined() {
            return;
        }

        match self.layout.get::<TextFont>() {
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
