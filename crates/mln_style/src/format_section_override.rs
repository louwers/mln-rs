// The functions that install `format-section-override` in a symbol layer's
// paint properties. The only overridable property is `text-color`.

use std::sync::Arc;

use mln_expression::format_section_override::{FormatSectionOverride, OverrideDefault};

use crate::expression::{Dependency, Expr, GlobalStateMap};
use crate::layers::symbol_layer_impl::SymbolLayerPaintPropertyOverrides;
use crate::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties, TextColor, TextField};
use crate::property_expression::PropertyExpression;
use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;

/// A `format-section-override` of the color property `property_name`, whose
/// value is `default_value` where no section overrides it.
pub(crate) fn format_section_override(
    default_value: PossiblyEvaluatedPropertyValue<mln_util::Color>,
    property_name: String,
) -> Expr {
    let default_value = match default_value {
        PossiblyEvaluatedPropertyValue::Constant(color) => OverrideDefault::Constant(color),
        PossiblyEvaluatedPropertyValue::Expression(expression) => OverrideDefault::Expression {
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
    if !SymbolLayerPaintPropertyOverrides::has_overrides(overrides.get::<TextField>()) {
        return;
    }
    let value = overridable.get_mut::<TextColor>();
    // Wrapping a folded constant would drop the GlobalState dependency from
    // the wrapper and exempt the bucket from relayout tracking.
    if value.is_constant() {
        let unevaluated_value = unevaluated.get::<TextColor>().get_value();
        if unevaluated_value.is_expression()
            && unevaluated_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
        {
            let mut retained = unevaluated_value.as_expression().clone();
            retained.capture_global_state(global_state.clone());
            *value = PossiblyEvaluatedPropertyValue::Expression(retained);
        }
    }
    let override_ = format_section_override(std::mem::take(value), "text-color".to_owned());
    *value = PossiblyEvaluatedPropertyValue::Expression(PropertyExpression::new(Arc::new(override_), None));
}
