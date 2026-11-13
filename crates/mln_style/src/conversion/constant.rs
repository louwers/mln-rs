// Ports include/mln/style/conversion/constant.hpp +
// src/mln/style/conversion/constant.cpp.

use std::str::FromStr;

use mln_style_types::{
    AlignmentType, AnchorOffsetPair, CirclePitchScaleType, HillshadeIlluminationAnchorType, HillshadeMethodType,
    IconTextFitType, LightAnchorType, LineCapType, LineJoinType, ProjectionDefinition, RasterResamplingType,
    ResamplingType, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType, TextJustifyType, TextTransformType,
    TextWritingModeType, TranslateAnchorType, VariableAnchorOffsetCollection, VisibilityType,
    projection_type_from_name,
};
use mln_util::{Color, Padding};

use super::{Converter, Convertible, Error, convert};

impl Converter for bool {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<bool> {
        let converted = value.to_bool();
        if converted.is_none() {
            error.message = "value must be a boolean".to_owned();
            return None;
        }
        converted
    }
}

impl Converter for f32 {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<f32> {
        let converted = value.to_number();
        if converted.is_none() {
            error.message = "value must be a number".to_owned();
            return None;
        }
        converted
    }
}

impl Converter for String {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<String> {
        let converted = value.to_string();
        if converted.is_none() {
            error.message = "value must be a string".to_owned();
            return None;
        }
        converted
    }
}

/// `Converter<T, enable_if_t<is_enum_v<T>>>`.
fn convert_enum<T: FromStr>(value: &Convertible<'_>, error: &mut Error) -> Option<T> {
    let Some(string) = value.to_string() else {
        error.message = "value must be a string".to_owned();
        return None;
    };

    let result = string.parse().ok();
    if result.is_none() {
        error.message = "value must be a valid enumeration value".to_owned();
    }

    result
}

/// `Converter<std::vector<T>, enable_if_t<is_enum_v<T>>>`.
fn convert_enum_vector<T: FromStr>(value: &Convertible<'_>, error: &mut Error) -> Option<Vec<T>> {
    if !value.is_array() {
        error.message = "value must be an array".to_owned();
        return None;
    }

    let mut result = Vec::with_capacity(value.array_length());

    for i in 0..value.array_length() {
        let enum_item = convert_enum::<T>(&value.array_member(i), error)?;
        result.push(enum_item);
    }

    Some(result)
}

// The explicit instantiations in constant.cpp.
macro_rules! enum_converters {
    ($($t:ty),* $(,)?) => {
        $(impl Converter for $t {
            fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<$t> {
                convert_enum::<$t>(value, error)
            }
        })*
    };
}

enum_converters!(
    AlignmentType,
    CirclePitchScaleType,
    HillshadeIlluminationAnchorType,
    HillshadeMethodType,
    IconTextFitType,
    LightAnchorType,
    LineCapType,
    LineJoinType,
    RasterResamplingType,
    ResamplingType,
    SymbolAnchorType,
    SymbolPlacementType,
    SymbolZOrderType,
    TextJustifyType,
    TextTransformType,
    TranslateAnchorType,
    VisibilityType,
);

impl Converter for Vec<SymbolAnchorType> {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        convert_enum_vector::<SymbolAnchorType>(value, error)
    }
}

impl Converter for Vec<TextWritingModeType> {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        convert_enum_vector::<TextWritingModeType>(value, error)
    }
}

impl Converter for Color {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Color> {
        let Some(string) = value.to_string() else {
            error.message = "value must be a string".to_owned();
            return None;
        };

        let color = Color::parse(&string);
        if color.is_none() {
            error.message = "value must be a valid color".to_owned();
            return None;
        }

        color
    }
}

impl Converter for Vec<Color> {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        if !value.is_array() {
            // Try single color
            let color = convert::<Color>(value, error)?;
            return Some(vec![color]);
        }

        let length = value.array_length();
        let mut result = Vec::with_capacity(length);

        for i in 0..length {
            let color = convert::<Color>(&value.array_member(i), error)?;
            result.push(color);
        }

        Some(result)
    }
}

impl Converter for ProjectionDefinition {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<ProjectionDefinition> {
        if let Some(name) = value.to_string() {
            if let Some(type_) = projection_type_from_name(&name) {
                return Some(ProjectionDefinition::new(type_));
            }
            error.message = "projection type must be mercator, vertical-perspective or globe".to_owned();
            return None;
        }
        if value.is_array() && value.array_length() == 3 {
            let from_name = value.array_member(0).to_string();
            let to_name = value.array_member(1).to_string();
            let transition = value.array_member(2).to_double();
            if let (Some(from_name), Some(to_name), Some(transition)) = (from_name, to_name, transition) {
                let from = projection_type_from_name(&from_name);
                let to = projection_type_from_name(&to_name);
                if let (Some(from), Some(to)) = (from, to) {
                    return Some(ProjectionDefinition::between(from, to, transition));
                }
                error.message = "projection type must be mercator, vertical-perspective or globe".to_owned();
                return None;
            }
        }
        error.message = "value must be a string or an array of [from, to, transition]".to_owned();
        None
    }
}

impl Converter for Padding {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Padding> {
        let mut result = None;
        if value.is_array() {
            if value.array_length() > 0
                && value.array_length() <= 4
                && let Some(vector) = <Vec<f32> as Converter>::convert(value, error)
            {
                // `Padding(*vector)`: 1..=4 elements, checked above.
                result = Padding::from_values(&vector).ok();
            }
        } else if let Some(number) = value.to_number() {
            result = Some(Padding::uniform(number));
        }

        if result.is_none() {
            error.message = "value must be a number or an array of numbers (between 1 and 4 elements)".to_owned();
        }
        result
    }
}

impl Converter for VariableAnchorOffsetCollection {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        if !value.is_array() {
            error.message = "value must be an array".to_owned();
            return None;
        }

        let array_size = value.array_length();
        if array_size < 1 || !array_size.is_multiple_of(2) {
            error.message = "array must contain an even number of elements".to_owned();
            return None;
        }

        let mut collection = Vec::with_capacity(array_size / 2);
        let mut index = 0;
        while index < array_size {
            let offset_value = value.array_member(index + 1);
            let anchor = <SymbolAnchorType as Converter>::convert(&value.array_member(index), error);

            let Some(anchor) = anchor else {
                error.message = "anchor must be a valid anchor value".to_owned();
                return None;
            };

            if !offset_value.is_array() {
                error.message = "anchor offset must be an array".to_owned();
                return None;
            }

            if offset_value.array_length() != 2 {
                error.message = "anchor offset must have two elements".to_owned();
                return None;
            }

            let x_offset = offset_value.array_member(0).to_number();
            let y_offset = offset_value.array_member(1).to_number();
            let (Some(x_offset), Some(y_offset)) = (x_offset, y_offset) else {
                error.message = "anchor offset must have two numbers".to_owned();
                return None;
            };

            collection.push(AnchorOffsetPair::new(anchor, [x_offset, y_offset]));
            index += 2;
        }

        Some(VariableAnchorOffsetCollection::new(collection))
    }
}

/// `util::toString(N)` for the array messages.
fn array_message(n: usize) -> String {
    format!("value must be an array of {n} numbers")
}

impl<const N: usize> Converter for [f32; N] {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        if !value.is_array() || value.array_length() != N {
            error.message = array_message(N);
            return None;
        }

        let mut result = [0.0_f32; N];
        for (i, slot) in result.iter_mut().enumerate() {
            let Some(n) = value.array_member(i).to_number() else {
                error.message = array_message(N);
                return None;
            };
            *slot = n;
        }
        Some(result)
    }
}

impl<const N: usize> Converter for [f64; N] {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        if !value.is_array() || value.array_length() != N {
            error.message = array_message(N);
            return None;
        }

        let mut result = [0.0_f64; N];
        for (i, slot) in result.iter_mut().enumerate() {
            let Some(n) = value.array_member(i).to_double() else {
                error.message = array_message(N);
                return None;
            };
            *slot = n;
        }
        Some(result)
    }
}

impl Converter for Vec<f32> {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        // Allow single number to be converted to a single-element array
        if let Some(number) = value.to_number() {
            return Some(vec![number]);
        }

        if !value.is_array() {
            error.message = "value must be a number or an array of numbers".to_owned();
            return None;
        }

        let mut result = Vec::with_capacity(value.array_length());

        for i in 0..value.array_length() {
            let Some(number) = value.array_member(i).to_number() else {
                error.message = "value must be an array of numbers".to_owned();
                return None;
            };
            result.push(number);
        }

        Some(result)
    }
}

impl Converter for Vec<String> {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        if !value.is_array() {
            error.message = "value must be an array".to_owned();
            return None;
        }

        let mut result = Vec::with_capacity(value.array_length());

        for i in 0..value.array_length() {
            let Some(string) = value.array_member(i).to_string() else {
                error.message = "value must be an array of strings".to_owned();
                return None;
            };
            result.push(string);
        }

        Some(result)
    }
}

/// `Converter<expression::Formatted>` (src/mln/style/expression/formatted.cpp;
/// `mln_expression` ports it as `formatted::convert_formatted`).
impl Converter for mln_expression::formatted::Formatted {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Self> {
        mln_expression::formatted::convert_formatted(value, error)
    }
}
