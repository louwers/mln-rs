use std::fmt;

use crate::color::Color;
use crate::conversion::{Convertible, Error};
use crate::expression::image::Image;
use crate::types::VerticalAlign;

pub(crate) const K_FORMATTED_SECTION_FONT_SCALE: &str = "font-scale";
pub(crate) const K_FORMATTED_SECTION_TEXT_FONT: &str = "text-font";
pub const K_FORMATTED_SECTION_TEXT_COLOR: &str = "text-color";
pub(crate) const K_FORMATTED_SECTION_VERTICAL_ALIGN: &str = "vertical-align";

/// A list of font names.
pub(crate) type FontStack = Vec<String>;

#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct FormattedSection {
    pub text: String,
    pub image: Option<Image>,
    pub font_scale: Option<f64>,
    pub font_stack: Option<FontStack>,
    pub text_color: Option<Color>,
    /// Where the text or image sits in its line. `None` is
    /// [`VerticalAlign::Bottom`].
    pub vertical_align: Option<VerticalAlign>,
}

impl FormattedSection {
    #[must_use]
    pub fn new(
        text: String,
        font_scale: Option<f64>,
        font_stack: Option<FontStack>,
        text_color: Option<Color>,
    ) -> Self {
        FormattedSection { text, image: None, font_scale, font_stack, text_color, vertical_align: None }
    }

    #[must_use]
    pub fn from_image(image: Image) -> Self {
        FormattedSection { image: Some(image), ..Default::default() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct Formatted {
    pub sections: Vec<FormattedSection>,
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

/// Converts a plain string. A `text-field` array is a `["format", ...]`
/// expression, which the property value conversion parses before this.
pub fn convert_formatted(value: &Convertible<'_>, error: &mut Error) -> Option<Formatted> {
    if let Some(result) = value.to_string() {
        Some(Formatted::from_plain(&result))
    } else {
        error.message = "value must be a string".to_owned();
        None
    }
}
