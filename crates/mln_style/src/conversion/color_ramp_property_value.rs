use std::sync::Arc;

use super::{Convertible, Error};
use crate::color_ramp_property_value::ColorRampPropertyValue;
use crate::expression::{Dependency, ParsingContext, Type, is_expression};

/// Converts a color ramp property value. The two flags are ignored, as in
/// C++.
pub(crate) fn convert_color_ramp_property_value(
    value: &Convertible<'_>,
    error: &mut Error,
    _allow_data_expressions: bool,
    _convert_tokens: bool,
) -> Option<ColorRampPropertyValue> {
    if value.is_undefined() {
        Some(ColorRampPropertyValue::default())
    } else if is_expression(value) {
        let mut ctx = ParsingContext::with_expected(Type::Color);
        let Some(expression) = ctx.parse_layer_property_expression(value) else {
            error.message = ctx.get_combined_errors();
            return None;
        };

        let deps = expression.dependencies();
        if deps.intersects(Dependency::FEATURE) && !deps.intersects(Dependency::ELEVATION) {
            error.message = "data expressions not supported".to_owned();
            return None;
        }
        if deps.intersects(Dependency::ZOOM) {
            error.message = "zoom expressions not supported".to_owned();
            return None;
        }

        Some(ColorRampPropertyValue::new(Some(Arc::from(expression))))
    } else {
        error.message = "color ramp must be an expression".to_owned();
        None
    }
}
