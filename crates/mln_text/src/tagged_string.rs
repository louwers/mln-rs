use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use mln_util::font_stack::FontStack;
use mln_util::logging::{Event, Log};
use mln_util::{Color, i18n};

use crate::glyph::GlyphIDType;
use crate::harfbuzz::HBShapeAdjust;

/// Each UTF-16 code unit with the index of its section.
pub type StyledText = (Vec<u16>, Vec<u8>);

pub type FontStackHash = u64;

/// A key for a font stack in the glyph maps: [`std::hash`] of its font
/// names. Every thread and worker of one build computes the same key.
#[must_use]
pub fn font_stack_hash(font_stack: &FontStack) -> FontStackHash {
    let mut hasher = DefaultHasher::new();
    font_stack.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Debug)]
pub struct SectionOptions {
    pub scale: f64,
    pub font_stack: FontStack,
    pub font_stack_hash: FontStackHash,
    pub type_: GlyphIDType,
    pub adjusts: Option<Arc<Vec<HBShapeAdjust>>>,
    pub start_index: i32,
    pub key_section: bool,
    /// Whether a section of text that HarfBuzz shaped runs right to left.
    /// Its glyphs are in visual order, so bidi reorders the section as one
    /// block.
    pub rtl: bool,
    pub image_id: Option<String>,
    pub text_color: Option<Color>,
}

impl SectionOptions {
    #[must_use]
    pub fn new(
        scale: f64,
        font_stack: FontStack,
        type_: GlyphIDType,
        start_index: u32,
        text_color: Option<Color>,
    ) -> Self {
        let font_stack_hash = font_stack_hash(&font_stack);
        SectionOptions {
            scale,
            font_stack,
            font_stack_hash,
            type_,
            adjusts: None,
            start_index: start_index as i32,
            key_section: true,
            rtl: false,
            image_id: None,
            text_color,
        }
    }

    /// The options of an image section. The font stack hash and start index
    /// are 0 (C++ leaves them uninitialized).
    #[must_use]
    pub fn image(image_id: String) -> Self {
        SectionOptions {
            scale: 1.0,
            font_stack: FontStack::new(),
            font_stack_hash: 0,
            type_: GlyphIDType::FontPBF,
            adjusts: None,
            start_index: 0,
            key_section: true,
            rtl: false,
            image_id: Some(image_id),
            text_color: None,
        }
    }
}

/// Max number of images within a text is 6400 U+E000–U+F8FF that covers
/// Basic Multilingual Plane Unicode Private Use Area (PUA).
const PUA_BEGIN: u16 = 0xE000;
const PUA_END: u16 = 0xF8FF;

/// The shaping-code counterpart of `Formatted`, the text at a per-character
/// level so that character-rearranging operations (e.g. BiDi) preserve
/// formatting.
#[derive(Clone, Debug, Default)]
pub struct TaggedString {
    styled_text: StyledText,
    sections: Vec<SectionOptions>,
    supports_vertical_writing_mode: Option<bool>,
    image_section_id: u16,
    text_hb_shaped: bool,
    has_need_shape_text_val: bool,
}

/// `u" \t\n\v\f\r"`
fn is_trim_whitespace(c: u16) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D)
}

impl TaggedString {
    #[must_use]
    pub fn new(text: Vec<u16>, options: SectionOptions) -> Self {
        let size = text.len();
        TaggedString { styled_text: (text, vec![0; size]), sections: vec![options], ..Default::default() }
    }

    #[must_use]
    pub fn from_styled_text(styled_text: StyledText, sections: Vec<SectionOptions>) -> Self {
        TaggedString { styled_text, sections, ..Default::default() }
    }

    #[must_use]
    pub fn length(&self) -> usize {
        self.styled_text.0.len()
    }

    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    #[must_use]
    pub fn empty(&self) -> bool {
        self.styled_text.0.is_empty()
    }

    /// The section of the code unit at `index`. Panics when out of range.
    #[must_use]
    pub fn get_section(&self, index: usize) -> &SectionOptions {
        &self.sections[self.styled_text.1[index] as usize]
    }

    #[must_use]
    pub fn get_char_code_at(&self, index: usize) -> u16 {
        self.styled_text.0[index]
    }

    #[must_use]
    pub fn raw_text(&self) -> &[u16] {
        &self.styled_text.0
    }

    #[must_use]
    pub fn get_styled_text(&self) -> &StyledText {
        &self.styled_text
    }

    /// Appends a text section. Quirk kept from C++: the section's start index
    /// is the length *after* appending the text.
    pub fn add_text_section(
        &mut self,
        section_text: &[u16],
        scale: f64,
        font_stack: &FontStack,
        type_: GlyphIDType,
        key_section: bool,
        text_color: Option<Color>,
    ) {
        self.styled_text.0.extend_from_slice(section_text);
        let start_index = self.styled_text.0.len() as u32;
        self.sections.push(SectionOptions::new(scale, font_stack.clone(), type_, start_index, text_color));
        let index = (self.sections.len() - 1) as u8;
        self.styled_text.1.resize(self.styled_text.0.len(), index);
        self.supports_vertical_writing_mode = None;
        if type_ != GlyphIDType::FontPBF {
            self.has_need_shape_text_val = true;
        }
        self.sections.last_mut().unwrap().key_section = key_section;
    }

    /// Appends a text section with HarfBuzz adjustments. The section's start
    /// index is the length before the text.
    #[allow(clippy::too_many_arguments)]
    pub fn add_text_section_with_adjusts(
        &mut self,
        section_text: &[u16],
        scale: f64,
        font_stack: &FontStack,
        type_: GlyphIDType,
        adjusts: &Option<Arc<Vec<HBShapeAdjust>>>,
        rtl: bool,
        key_section: bool,
        text_color: Option<Color>,
    ) {
        let start_index = self.styled_text.0.len() as u32;
        self.sections.push(SectionOptions::new(scale, font_stack.clone(), type_, start_index, text_color));
        self.styled_text.0.extend_from_slice(section_text);
        let index = (self.sections.len() - 1) as u8;
        self.styled_text.1.resize(self.styled_text.0.len(), index);
        if type_ != GlyphIDType::FontPBF {
            self.has_need_shape_text_val = true;
        }
        if let Some(adjusts) = adjusts {
            self.sections.last_mut().unwrap().adjusts = Some(adjusts.clone());
        }
        let section = self.sections.last_mut().unwrap();
        section.rtl = rtl;
        section.key_section = key_section;
    }

    pub fn add_image_section(&mut self, image_id: &str) {
        let Some(next) = self.get_next_image_section_char_code() else {
            Log::warning(Event::Style, "Exceeded maximum number of images in a label.");
            return;
        };
        self.styled_text.0.push(next);
        self.sections.push(SectionOptions::image(image_id.to_string()));
        let index = (self.sections.len() - 1) as u8;
        self.styled_text.1.resize(self.styled_text.0.len(), index);
    }

    #[must_use]
    pub fn section_at(&self, index: usize) -> &SectionOptions {
        &self.sections[index]
    }

    #[must_use]
    pub fn get_sections(&self) -> &[SectionOptions] {
        &self.sections
    }

    #[must_use]
    pub fn get_section_index(&self, character_index: usize) -> u8 {
        self.styled_text.1[character_index]
    }

    /// The largest section scale, compared with `fmax`.
    #[must_use]
    pub fn get_max_scale(&self) -> f64 {
        let mut max_scale = 0.0;
        for i in 0..self.styled_text.0.len() {
            max_scale = mln_math::fmax(max_scale, self.get_section(i).scale);
        }
        max_scale
    }

    /// `trim`: removes leading and trailing " \t\n\v\f\r", but not from
    /// sections that need shaping.
    pub fn trim(&mut self) {
        let text = &self.styled_text.0;
        // find_first_not_of (npos = None)
        let mut beginning_whitespace = text.iter().position(|&c| !is_trim_whitespace(c));
        let limit = beginning_whitespace.unwrap_or(usize::MAX);
        let mut i = 0;
        while i < limit && i < text.len() {
            if self.get_section(i).type_ != GlyphIDType::FontPBF {
                beginning_whitespace = Some(i);
                break;
            }
            i += 1;
        }

        match beginning_whitespace {
            None => {
                for section in &mut self.sections {
                    section.start_index = 0;
                }
                // Entirely whitespace
                self.styled_text.0.clear();
                self.styled_text.1.clear();
            }
            Some(beginning_whitespace) => {
                // find_last_not_of + 1 (npos + 1 == 0)
                let mut trailing_whitespace =
                    text.iter().rposition(|&c| !is_trim_whitespace(c)).map_or(0, |p| p + 1) as i32;

                if beginning_whitespace != 0 {
                    for section in &mut self.sections {
                        section.start_index = (section.start_index as usize).wrapping_sub(beginning_whitespace) as i32;
                    }
                }

                let mut i = self.styled_text.0.len() as i32 - 1;
                while i >= trailing_whitespace {
                    if self.get_section(i as usize).type_ != GlyphIDType::FontPBF {
                        trailing_whitespace = i + 1;
                        break;
                    }
                    i -= 1;
                }

                let end = trailing_whitespace as usize;
                self.styled_text.0 = self.styled_text.0[beginning_whitespace..end].to_vec();
                self.styled_text.1 = self.styled_text.1[beginning_whitespace..end].to_vec();
            }
        }
    }

    /// Replaces punctuation with its vertical forms. Relies on
    /// verticalization changing characters in place so that style indices
    /// don't need updating.
    pub fn verticalize_punctuation(&mut self) {
        let mut replaced = i18n::verticalize_punctuation_str(&self.styled_text.0);
        for (i, unit) in replaced.iter_mut().enumerate() {
            if self.get_section(i).type_ != GlyphIDType::FontPBF {
                *unit = self.styled_text.0[i];
            }
        }
        self.styled_text.0 = replaced;
    }

    /// Whether the text allows vertical writing mode (cached until the next
    /// section is added).
    pub fn allows_vertical_writing_mode(&mut self) -> bool {
        if self.supports_vertical_writing_mode.is_none() {
            let mut allows = false;
            for i in 0..self.styled_text.0.len() {
                let chr = self.styled_text.0[i];
                if self.get_section(i).type_ == GlyphIDType::FontPBF && i18n::has_upright_vertical_orientation(chr) {
                    allows = true;
                    break;
                }
            }
            self.supports_vertical_writing_mode = Some(allows);
        }
        self.supports_vertical_writing_mode.unwrap()
    }

    #[must_use]
    pub fn hb_shaped(&self) -> bool {
        self.text_hb_shaped
    }

    pub fn set_hb_shaped(&mut self, shaped: bool) {
        self.text_hb_shaped = shaped;
    }

    #[must_use]
    pub fn has_need_hb_shape_text(&self) -> bool {
        self.has_need_shape_text_val
    }

    fn get_next_image_section_char_code(&mut self) -> Option<u16> {
        if self.image_section_id == 0 {
            self.image_section_id = PUA_BEGIN;
            return Some(self.image_section_id);
        }
        self.image_section_id = self.image_section_id.wrapping_add(1);
        if self.image_section_id > PUA_END {
            return None;
        }
        Some(self.image_section_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    // gtest: TaggedString.Trim
    #[test]
    fn trim() {
        let mut basic = TaggedString::new(
            u(" \t\ntrim that and not this  \n\t"),
            SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, 0, None),
        );
        basic.trim();
        assert_eq!(basic.raw_text(), u("trim that and not this"));

        let mut two_sections = TaggedString::default();
        two_sections.add_text_section(&u(" \t\ntrim that"), 1.5, &vec![], GlyphIDType::FontPBF, false, None);
        two_sections.add_text_section(&u(" and not this  \n\t"), 0.5, &vec![], GlyphIDType::FontPBF, false, None);

        two_sections.trim();
        assert_eq!(two_sections.raw_text(), u("trim that and not this"));

        let mut empty =
            TaggedString::new(u("\n\t\u{b} \r  \t\n"), SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, 0, None));
        empty.trim();
        assert_eq!(empty.raw_text(), u(""));

        let mut no_trim =
            TaggedString::new(u("no trim!"), SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, 0, None));
        no_trim.trim();
        assert_eq!(no_trim.raw_text(), u("no trim!"));
    }

    // gtest: TaggedString.ImageSections
    #[test]
    fn image_sections() {
        let mut string = TaggedString::default();
        string.add_image_section("image_name");
        assert_eq!(string.raw_text(), u("\u{E000}"));
        assert!(string.get_section(0).image_id.is_some());
        assert_eq!(string.get_section(0).image_id.as_deref(), Some("image_name"));

        let mut max_sections = TaggedString::default();
        for i in 0..6401 {
            max_sections.add_image_section(&i.to_string());
        }

        assert_eq!(max_sections.get_sections().len(), 6400);
        assert_eq!(max_sections.get_char_code_at(0), 0xE000);
        assert_eq!(max_sections.get_char_code_at(6399), 0xF8FF);
    }

    #[test]
    fn font_stack_hash_tells_stacks_apart() {
        let stack = |fonts: &[&str]| fonts.iter().map(|f| (*f).to_string()).collect::<FontStack>();
        assert_eq!(font_stack_hash(&stack(&["Open Sans", "Arial"])), font_stack_hash(&stack(&["Open Sans", "Arial"])));
        assert_ne!(font_stack_hash(&stack(&["Open Sans", "Arial"])), font_stack_hash(&stack(&["Arial", "Open Sans"])));
        assert_ne!(font_stack_hash(&stack(&["a", "b"])), font_stack_hash(&stack(&["ab"])));
        assert_ne!(font_stack_hash(&stack(&["a", "b"])), font_stack_hash(&stack(&["a", "b", ""])));
    }
}
