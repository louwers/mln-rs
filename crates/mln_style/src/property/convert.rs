use std::sync::Arc;

use crate::color::Color;
use crate::conversion::{Converter, Convertible, Error, convert};
use crate::expression::{Functions, ParsingContext, Type, ValueConverter, is_expression};
use crate::property::expression::PropertyExpression;
use crate::property::function::{has_tokens, parse_converted, parse_function, token_string_expression};
use crate::property::value::PropertyValue;

/// Converts a property's value: a constant, an expression, or a legacy
/// function or (with `convert_tokens`) `{token}` string, which become the
/// expressions they stand for. Expressions parse against `functions`.
pub fn convert_property_value<T>(
    value: &Convertible<'_>,
    error: &mut Error,
    allow_data_expressions: bool,
    convert_tokens: bool,
    functions: &Arc<Functions>,
) -> Option<PropertyValue<T>>
where
    T: Converter + ValueConverter + Clone,
{
    if value.is_undefined() {
        return Some(PropertyValue::Undefined);
    }

    let mut default_value = None;
    let parsed = if is_expression(value, functions) {
        let mut ctx = ParsingContext::with_expected(functions, T::expression_type());
        let parsed = ctx.parse_layer_property_expression(value);
        if parsed.is_none() {
            error.message = ctx.get_combined_errors();
        }
        parsed?
    } else if value.is_object() {
        // A function of a color array, a hillshade's, takes single colors.
        let type_ = match T::expression_type() {
            Type::Array(array) if array.item_type == Type::Color => Color::expression_type(),
            type_ => type_,
        };
        if let Some(default) = value.object_member("default") {
            let Some(default) = convert::<T>(&default, error) else {
                error.message = format!(r#"wrong type for "default": {}"#, error.message);
                return None;
            };
            default_value = Some(default);
        }
        parse_function(value, &type_, convert_tokens, functions, error)?
    } else if let Some(string) = value.to_string().filter(|s| convert_tokens && has_tokens(s)) {
        parse_converted(&token_string_expression(&string), &T::expression_type(), functions, error)?
    } else {
        return Some(PropertyValue::Constant(convert::<T>(value, error)?));
    };
    let expression = PropertyExpression::<T>::from_expr(parsed).with_default(default_value);
    if !allow_data_expressions && !expression.is_feature_constant() {
        error.message = "data expressions not supported".to_owned();
        None
    } else if !expression.is_feature_constant() || !expression.is_zoom_constant() || !expression.is_runtime_constant() {
        Some(PropertyValue::Expression(expression))
    } else if let Some(literal) = expression.get_expression().literal_value() {
        let constant = T::from_expression_value(literal)?;
        Some(PropertyValue::Constant(constant))
    } else {
        debug_assert!(false, "expected a literal expression");
        error.message = "expected a literal expression".to_owned();
        None
    }
}
