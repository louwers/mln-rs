// Ports include/mln/style/expression/format_section_override.hpp and the
// `setOverrides`/`setOverride` half of `FormatSectionOverrides`
// (src/mln/style/layers/symbol_layer_impl.hpp). In mln_style, with the
// `PossiblyEvaluatedPropertyValue` it wraps (renderer/, a cut point). The
// only overridable property is `text-color`, so the template is instantiated
// for `Color`.

use std::sync::Arc;

use mln_expression::expression::{
    Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use mln_expression::type_::Type;
use mln_expression::value::{Value, ValueConverter};
use mln_util::Color;

use crate::expression::GlobalStateMap;
use crate::layers::symbol_layer_impl::SymbolLayerPaintPropertyOverrides;
use crate::layers::symbol_layer_properties::{SymbolLayoutProperties, SymbolPaintProperties, TextColor, TextField};
use crate::property_expression::PropertyExpression;
use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;

/// `FormatSectionOverride<Color>`.
pub struct FormatSectionOverride {
    base: ExpressionBase,
    default_value: PossiblyEvaluatedPropertyValue<Color>,
    property_name: String,
}

impl FormatSectionOverride {
    #[must_use]
    pub fn new(type_: Type, default_value: PossiblyEvaluatedPropertyValue<Color>, property_name: String) -> Self {
        let dependencies = default_value.get_dependencies() | Dependency::OVERRIDE | Dependency::FEATURE;
        FormatSectionOverride {
            base: ExpressionBase::new(Kind::FormatSectionOverride, type_, dependencies),
            default_value,
            property_name,
        }
    }
}

impl FormatSectionOverride {
    /// Port-only: the default value and the property name, which serde
    /// writes for a tile worker in another wasm instance
    /// (serialization.rs).
    pub(crate) fn parts(&self) -> (&PossiblyEvaluatedPropertyValue<Color>, &str) {
        (&self.default_value, &self.property_name)
    }
}

impl Expression for FormatSectionOverride {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, context: &EvaluationContext<'_>) -> EvaluationResult {
        if let Some(Value::Object(section)) = context.formatted_section
            && let Some(hit) = section.get(&self.property_name)
        {
            return Ok(hit.clone());
        }
        match &self.default_value {
            PossiblyEvaluatedPropertyValue::Expression(e) => {
                // Binder contexts do not carry global state, so the captured
                // state is used as a fallback. If the caller explicitly
                // provides context state, it takes precedence.
                if context.global_state.is_none()
                    && let Some(state) = e.get_captured_global_state()
                {
                    let mut context_with_state = *context;
                    context_with_state.global_state = Some(state);
                    return e.get_expression().evaluate(&context_with_state);
                }
                e.get_expression().evaluate(context)
            }
            PossiblyEvaluatedPropertyValue::Constant(t) => Ok(t.to_expression_value()),
        }
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        if let PossiblyEvaluatedPropertyValue::Expression(e) = &self.default_value {
            visit(e.get_expression());
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        let Some(other) = downcast::<FormatSectionOverride>(e) else {
            return false;
        };
        if self.get_type() != other.get_type() || self.property_name != other.property_name {
            return false;
        }
        // Check that default values or property expressions are equal.
        match (&self.default_value, &other.default_value) {
            (PossiblyEvaluatedPropertyValue::Expression(a), PossiblyEvaluatedPropertyValue::Expression(b)) => a == b,
            (PossiblyEvaluatedPropertyValue::Constant(a), PossiblyEvaluatedPropertyValue::Constant(b)) => a == b,
            _ => false,
        }
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn get_operator(&self) -> String {
        "format-section-override".to_owned()
    }
}

/// `SymbolLayerPaintPropertyOverrides::setOverrides(layout, evaluated,
/// unevaluated, globalState)` (`setOverride<TextColor>`).
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
    let default_value = std::mem::take(value);
    let override_ = FormatSectionOverride::new(TextColor::expression_type(), default_value, "text-color".to_owned());
    *value = PossiblyEvaluatedPropertyValue::Expression(PropertyExpression::new(Arc::new(override_), None));
}
