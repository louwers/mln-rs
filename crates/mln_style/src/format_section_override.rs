// The functions that install `format-section-override` in a symbol layer's
// paint properties. The only overridable property is `text-color`.

use std::sync::Arc;

use crate::expression::forms::format_section_override::{FormatSectionOverride, OverrideDefault};
use crate::expression::{Dependency, Expr, GlobalStateMap};
use crate::layers::symbol_layer::SymbolLayerPaintPropertyOverrides;
use crate::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties};
use crate::property::evaluated::Evaluated;
use crate::property::expression::PropertyExpression;
use crate::property::value::StyleValue as _;

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

pub fn set_overrides(
    overrides: &SymbolLayoutProperties::Unevaluated,
    overridable: &mut SymbolPaintProperties::PossiblyEvaluated,
    unevaluated: &SymbolPaintProperties::Unevaluated,
    global_state: &Option<Arc<GlobalStateMap>>,
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
            retained.capture_global_state(global_state.clone());
            *value =
                Evaluated::PerFeature { expression: retained, default: SymbolPaintProperties::Defaults::text_color() };
        }
    }
    let override_ = format_section_override(std::mem::take(value), "text-color".to_owned());
    *value = Evaluated::PerFeature {
        expression: PropertyExpression::new(Arc::new(override_)),
        default: SymbolPaintProperties::Defaults::text_color(),
    };
}
