use std::fmt;

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::{Convertible, Error};
use mln_util::Color;
use serde::{Deserialize, Serialize};

use crate::image::Image;

pub const K_FORMATTED_SECTION_FONT_SCALE: &str = "font-scale";
pub const K_FORMATTED_SECTION_TEXT_FONT: &str = "text-font";
pub const K_FORMATTED_SECTION_TEXT_COLOR: &str = "text-color";

/// A list of font names.
pub type FontStack = Vec<String>;

#[must_use]
pub fn font_stack_to_string(font_stack: &FontStack) -> String {
    font_stack.join(",")
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FormattedSection {
    pub text: String,
    pub image: Option<Image>,
    pub font_scale: Option<f64>,
    pub font_stack: Option<FontStack>,
    pub text_color: Option<Color>,
}

impl FormattedSection {
    #[must_use]
    pub fn new(
        text: String,
        font_scale: Option<f64>,
        font_stack: Option<FontStack>,
        text_color: Option<Color>,
    ) -> Self {
        FormattedSection { text, image: None, font_scale, font_stack, text_color }
    }

    #[must_use]
    pub fn from_image(image: Image) -> Self {
        FormattedSection { image: Some(image), ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Formatted {
    pub sections: Vec<FormattedSection>,
}

impl PartialEq for Formatted {
    fn eq(&self, other: &Self) -> bool {
        if other.sections.len() != self.sections.len() {
            return false;
        }
        for (this_section, other_section) in self.sections.iter().zip(&other.sections) {
            if this_section.text != other_section.text
                || this_section.image != other_section.image
                || this_section.font_scale != other_section.font_scale
                || this_section.font_stack != other_section.font_stack
                || this_section.text_color != other_section.text_color
            {
                return false;
            }
        }
        true
    }
}

/// The sections' text, concatenated.
impl fmt::Display for Formatted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for section in &self.sections {
            f.write_str(&section.text)?;
        }
        Ok(())
    }
}

impl Formatted {
    #[must_use]
    pub fn from_plain(plain_u8_string: &str) -> Self {
        Formatted { sections: vec![FormattedSection::new(plain_u8_string.to_owned(), None, None, None)] }
    }

    #[must_use]
    pub fn new(sections: Vec<FormattedSection>) -> Self {
        Formatted { sections }
    }

    pub fn to_object(&self) -> MbglValue {
        let mut result = ValueObject::new();
        let mut section_values = Vec::with_capacity(self.sections.len());
        for section in &self.sections {
            let mut serialized_section = ValueObject::new();
            serialized_section.insert("text".to_owned(), MbglValue::String(section.text.clone()));
            serialized_section
                .insert("scale".to_owned(), section.font_scale.map_or(MbglValue::Null, MbglValue::Double));
            serialized_section.insert(
                "fontStack".to_owned(),
                section.font_stack.as_ref().map_or(MbglValue::Null, |f| MbglValue::String(font_stack_to_string(f))),
            );
            serialized_section
                .insert("textColor".to_owned(), section.text_color.as_ref().map_or(MbglValue::Null, Color::to_object));
            serialized_section
                .insert("image".to_owned(), section.image.as_ref().map_or(MbglValue::Null, Image::to_value));
            section_values.push(MbglValue::Object(serialized_section));
        }
        result.insert("sections".to_owned(), MbglValue::Array(section_values));
        MbglValue::Object(result)
    }

    #[must_use]
    pub fn empty(&self) -> bool {
        if self.sections.is_empty() {
            return true;
        }

        !self
            .sections
            .iter()
            .any(|section| !section.text.is_empty() || section.image.as_ref().is_some_and(|i| !i.empty()))
    }
}

/// Converts a color string, for the `Formatted` converter.
fn convert_color(value: &Convertible<'_>, error: &mut Error) -> Option<Color> {
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

pub fn convert_formatted(value: &Convertible<'_>, error: &mut Error) -> Option<Formatted> {
    if value.is_array() {
        let mut sections = Vec::new();
        for i in 0..value.array_length() {
            let section = value.array_member(i);
            let section_length = section.array_length();
            if section_length < 1 {
                error.message = "Section has to contain a text and std::optional parameters or an image.".to_owned();
                return None;
            }

            let first_element = section.array_member(0);
            if first_element.is_array() {
                if first_element.array_length() < 2 {
                    error.message = "Image section has to contain image name.".to_owned();
                    return None;
                }

                let image_op = first_element.array_member(0).to_string();
                if image_op.as_deref() != Some("image") {
                    error.message = "Serialized image section has to contain 'image' operator.".to_owned();
                    return None;
                }

                let Some(image_arg) = first_element.array_member(1).to_string() else {
                    error.message = "Serialized image section argument has to be of a String type.".to_owned();
                    return None;
                };

                sections.push(FormattedSection::from_image(Image::new(image_arg)));
                continue;
            }

            let Some(section_text) = first_element.to_string() else {
                error.message = "Section has to contain a text.".to_owned();
                return None;
            };

            let mut font_scale = None;
            let mut text_font = None;
            let mut text_color = None;
            if section_length > 1 {
                let section_params = section.array_member(1);
                if !section_params.is_object() {
                    error.message = "Parameters have to be enclosed in an object.".to_owned();
                    return None;
                }

                if let Some(font_scale_member) = section_params.object_member(K_FORMATTED_SECTION_FONT_SCALE) {
                    font_scale = font_scale_member.to_double();
                }

                if let Some(text_font_member) = section_params.object_member(K_FORMATTED_SECTION_TEXT_FONT) {
                    if text_font_member.is_array() {
                        let mut fonts_vector = Vec::new();
                        for j in 0..text_font_member.array_length() {
                            if let Some(font) = text_font_member.array_member(j).to_string() {
                                fonts_vector.push(font);
                            } else {
                                error.message = "Font has to be a string.".to_owned();
                                return None;
                            }
                        }
                        text_font = Some(fonts_vector);
                    } else {
                        error.message = "Font stack has to be an array.".to_owned();
                        return None;
                    }
                }

                if let Some(text_color_member) = section_params.object_member(K_FORMATTED_SECTION_TEXT_COLOR) {
                    text_color = convert_color(&text_color_member, error);
                    text_color?;
                }
            }

            sections.push(FormattedSection::new(section_text, font_scale, text_font, text_color));
        }
        Some(Formatted::new(sections))
    } else if let Some(result) = value.to_string() {
        Some(Formatted::from_plain(&result))
    } else {
        error.message = "Formatted must be plain string or array type.".to_owned();
        None
    }
}
