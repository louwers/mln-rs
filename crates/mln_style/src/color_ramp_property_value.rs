use std::sync::Arc;

use mln_util::Color;
use mln_vec_map::VecSet;

use crate::conversion::{Convertible, Error, convert_color_ramp_property_value};
use crate::conversion_impl::make_color_ramp_style_property;
use crate::evaluated::{Evaluated, Evaluation};
use crate::expression::value::from_expression_value;
use crate::expression::{Dependency, EvaluationContext, Expr, Functions, GlobalStateMap, collect_global_state_refs};
use crate::property_table::PropertyInfo;
use crate::property_value::StyleValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::serialization::optional_expression;
use crate::style_property::StyleProperty;

/// Special-case implementation of (a subset of) the `PropertyValue<T>`
/// interface for the paint properties of a color ramp, which a layer type
/// draws from a texture of the ramp.
///
/// Generic paint-property evaluation uses a placeholder Color, while the
/// render layers evaluate the expression separately to build their
/// color-ramp textures. Referenced global-state keys are retained so those
/// textures are rebuilt only when one of those keys changes.
///
/// Port-only: crosses as the expression; the global-state keys are
/// collected from it again.
#[derive(Clone, Debug, Default)]
pub struct ColorRampPropertyValue {
    value: Option<Arc<Expr>>,
    global_state_refs: Option<Arc<VecSet<String>>>,
}

impl PartialEq for ColorRampPropertyValue {
    fn eq(&self, other: &Self) -> bool {
        (self.is_undefined() && other.is_undefined())
            || match (&self.value, &other.value) {
                (Some(lhs), Some(rhs)) => lhs == rhs,
                _ => false,
            }
    }
}

impl mln_wire::Wire for ColorRampPropertyValue {
    fn write(&self, w: &mut mln_wire::Writer) {
        optional_expression::write(&self.value, w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        optional_expression::read(r).map(ColorRampPropertyValue::new)
    }
}

impl ColorRampPropertyValue {
    #[must_use]
    pub(crate) fn new(value: Option<Arc<Expr>>) -> Self {
        let global_state_refs = match &value {
            Some(expression) if expression.has(Dependency::GLOBAL_STATE) => {
                let mut refs = VecSet::new();
                collect_global_state_refs(expression.as_ref(), &mut refs);
                Some(Arc::new(refs))
            }
            _ => None,
        };
        ColorRampPropertyValue { value, global_state_refs }
    }

    /// The ramp's color at `ramp_evaluation_parameter`, or the default color
    /// when the value is undefined or doesn't evaluate to a color.
    #[must_use]
    pub fn evaluate_ramp(&self, ramp_evaluation_parameter: f64, global_state: Option<&GlobalStateMap>) -> Color {
        let Some(value) = &self.value else {
            return Color::default();
        };
        let result = value.evaluate(
            &EvaluationContext::with_color_ramp(None, None, Some(ramp_evaluation_parameter))
                .with_global_state(global_state),
        );
        let Ok(result) = result else {
            return Color::default();
        };
        from_expression_value::<Color>(&result).unwrap_or_default()
    }

    /// The expression, or `None` when the value is undefined.
    #[must_use]
    pub fn get_expression(&self) -> Option<&Expr> {
        self.value.as_deref()
    }
}

impl StyleValue for ColorRampPropertyValue {
    type Type = Color;

    /// A color ramp is always an expression, so `info`'s flags don't apply.
    fn convert(value: &Convertible<'_>, _info: &PropertyInfo, functions: &Arc<Functions>) -> Result<Self, Error> {
        let mut error = Error::default();
        convert_color_ramp_property_value(value, &mut error, functions).ok_or(error)
    }

    fn style_property(&self) -> StyleProperty {
        make_color_ramp_style_property(self)
    }

    /// The default: a layer builds its ramp from the expression itself.
    fn evaluate(&self, _: Evaluation<Color>, _: &PropertyEvaluationParameters, default: Color) -> Evaluated<Color> {
        Evaluated::Constant(default)
    }

    fn is_undefined(&self) -> bool {
        self.value.is_none()
    }

    fn is_data_driven(&self) -> bool {
        false
    }

    fn get_dependencies(&self) -> Dependency {
        match &self.value {
            Some(value) => value.dependencies(),
            None => Dependency::NONE,
        }
    }

    fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        self.global_state_refs.as_deref()
    }
}
