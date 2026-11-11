// Ports include/mln/style/conversion/property_value.hpp +
// src/mln/style/conversion/property_value.cpp.

use mln_expression::formatted::Formatted;
use mln_style_types::{
    AlignmentType, CirclePitchScaleType, HillshadeIlluminationAnchorType, HillshadeMethodType, IconTextFitType,
    LightAnchorType, LineCapType, LineJoinType, Position, ProjectionDefinition, RasterResamplingType, Rotation,
    SymbolAnchorType, SymbolPlacementType, SymbolZOrderType, TextJustifyType, TextTransformType, TextWritingModeType,
    TranslateAnchorType, VariableAnchorOffsetCollection, VisibilityType,
};
use mln_util::{Color, Padding};

use super::function::{
    convert_function_to_expression, convert_token_string_to_expression, convert_token_string_to_format_expression,
    convert_token_string_to_image_expression, has_tokens,
};
use super::{Converter, Convertible, Error, convert};
use crate::expression::{Image, Kind, Literal, ParsingContext, ValueConverter, downcast, is_expression};
use crate::property_expression::PropertyExpression;
use crate::property_value::PropertyValue;

/// `Converter<PropertyValue<T>>::maybeConvertTokens`: only strings,
/// `Formatted` and `Image` values can hold `{token}`s.
pub trait MaybeConvertTokens: Sized {
    fn maybe_convert_tokens(self) -> PropertyValue<Self> {
        PropertyValue::Constant(self)
    }
}

macro_rules! no_tokens {
    ($($t:ty),* $(,)?) => {
        $(impl MaybeConvertTokens for $t {})*
    };
}

no_tokens!(
    bool,
    f32,
    Color,
    Padding,
    Position,
    ProjectionDefinition,
    Rotation,
    VariableAnchorOffsetCollection,
    [f32; 2],
    [f32; 4],
    Vec<f32>,
    Vec<String>,
    Vec<Color>,
    Vec<SymbolAnchorType>,
    Vec<TextWritingModeType>,
    AlignmentType,
    CirclePitchScaleType,
    HillshadeIlluminationAnchorType,
    HillshadeMethodType,
    IconTextFitType,
    LightAnchorType,
    LineCapType,
    LineJoinType,
    RasterResamplingType,
    SymbolAnchorType,
    SymbolPlacementType,
    SymbolZOrderType,
    TextJustifyType,
    TextTransformType,
    TranslateAnchorType,
    VisibilityType,
);

impl MaybeConvertTokens for String {
    fn maybe_convert_tokens(self) -> PropertyValue<String> {
        if has_tokens(&self) {
            PropertyValue::Expression(PropertyExpression::from_box(convert_token_string_to_expression(&self), None))
        } else {
            PropertyValue::Constant(self)
        }
    }
}

impl MaybeConvertTokens for Formatted {
    fn maybe_convert_tokens(self) -> PropertyValue<Formatted> {
        // This only works with a single-section `Formatted` created automatically
        // by parsing a plain-text `text-field` property.
        // Token conversion happens later than the initial string->Formatted conversion
        // General purpose `format` expressions with embedded tokens are not supported
        let first_unformatted_section = &self.sections[0].text;
        if has_tokens(first_unformatted_section) {
            PropertyValue::Expression(PropertyExpression::from_box(
                convert_token_string_to_format_expression(first_unformatted_section),
                None,
            ))
        } else {
            PropertyValue::Constant(self)
        }
    }
}

impl MaybeConvertTokens for Image {
    fn maybe_convert_tokens(self) -> PropertyValue<Image> {
        if has_tokens(self.id()) {
            PropertyValue::Expression(PropertyExpression::from_box(
                convert_token_string_to_image_expression(self.id()),
                None,
            ))
        } else {
            PropertyValue::Constant(self)
        }
    }
}

/// `convert<PropertyValue<T>>(value, error, allowDataExpressions, convertTokens)`.
pub fn convert_property_value<T>(
    value: &Convertible<'_>,
    error: &mut Error,
    allow_data_expressions: bool,
    convert_tokens: bool,
) -> Option<PropertyValue<T>>
where
    T: Converter + MaybeConvertTokens + ValueConverter + Clone,
{
    if value.is_undefined() {
        return Some(PropertyValue::Undefined);
    }

    let expression: Option<PropertyExpression<T>>;

    if is_expression(value) {
        let mut ctx = ParsingContext::with_expected(T::expression_type());
        let Some(parsed) = ctx.parse_layer_property_expression(value) else {
            error.message = ctx.get_combined_errors();
            return None;
        };
        expression = Some(PropertyExpression::from_box(parsed, None));
    } else if value.is_object() {
        expression = convert_function_to_expression::<T>(value, error, convert_tokens);
    } else {
        let constant = convert::<T>(value, error)?;
        return Some(if convert_tokens { constant.maybe_convert_tokens() } else { PropertyValue::Constant(constant) });
    }

    let expression = expression?;
    if !allow_data_expressions && !expression.is_feature_constant() {
        error.message = "data expressions not supported".to_owned();
        None
    } else if !expression.is_feature_constant() || !expression.is_zoom_constant() || !expression.is_runtime_constant() {
        Some(PropertyValue::Expression(expression))
    } else if expression.get_expression().get_kind() == Kind::Literal {
        let literal = downcast::<Literal>(expression.get_expression()).expect("Kind::Literal is a Literal");
        let constant = T::from_expression_value(literal.get_value())?;
        Some(PropertyValue::Constant(constant))
    } else {
        debug_assert!(false, "expected a literal expression");
        error.message = "expected a literal expression".to_owned();
        None
    }
}

/// `Converter<PropertyValue<std::array<double, 3>>>`.
pub fn convert_property_value_double3(
    value: &Convertible<'_>,
    error: &mut Error,
    _allow_data_expressions: bool,
    _convert_tokens: bool,
) -> Option<PropertyValue<[f64; 3]>> {
    let a = convert::<[f64; 3]>(value, error)?;
    Some(PropertyValue::Constant([a[0], a[1], a[2]]))
}
