use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use mln_style::color::Color;
use mln_style::i18n;
use mln_style::types::VerticalAlign;
use mln_util::font_stack::FontStack;

use crate::log_target;
use crate::text::glyph::GlyphIDType;
use crate::text::shaper::ShapedText;

/// The index of a section of a [`TaggedString`]. A label in the device's
/// fonts takes a section for each word and each space.
pub type SectionIndex = u16;

/// Text as characters, with the index of each character's section. `text`
/// and `styles` are equally long.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyledText {
    pub text: Vec<char>,
    pub styles: Vec<SectionIndex>,
}

/// What the text of a shaped section holds for each of its glyphs: U+FFFC
/// OBJECT REPLACEMENT CHARACTER.
pub(crate) const SHAPED_GLYPH: char = '\u{FFFC}';

pub(crate) type FontStackHash = u64;

/// A key for a font stack in the glyph maps: [`std::hash`] of its font
/// names. Every thread and worker of one build computes the same key.
#[must_use]
pub fn font_stack_hash(font_stack: &FontStack) -> FontStackHash {
    let mut hasher = DefaultHasher::new();
    font_stack.hash(&mut hasher);
    hasher.finish()
}

/// The options of a section of a [`TaggedString`]: a run of text in one
/// font stack and style, or an inline image.
#[derive(Clone, Debug)]
pub struct SectionOptions {
    /// The section's font size, relative to the label's `text-size`.
    pub scale: f32,
    pub font_stack: FontStack,
    pub(crate) font_stack_hash: FontStackHash,
    pub type_: GlyphIDType,
    /// The glyphs of a section that a font face shaped. The section's text
    /// holds a [`SHAPED_GLYPH`] for each.
    pub(crate) shaped: Option<Arc<ShapedText>>,
    /// Whether the section is the last of a section of the style's
    /// `format` expression, rather than one of the runs it splits into.
    pub key_section: bool,
    /// Whether a section of text that a font shaped runs right to left.
    /// Its glyphs are in visual order, so bidi reorders the section as one
    /// block.
    pub(crate) rtl: bool,
    pub image_id: Option<String>,
    pub text_color: Option<Color>,
    /// Where the section sits in a horizontal line that's taller than it.
    pub vertical_align: VerticalAlign,
}

impl SectionOptions {
    /// The options of a key section of text, aligned to the bottom of its
    /// line.
    #[must_use]
    pub fn new(scale: f32, font_stack: FontStack, type_: GlyphIDType, text_color: Option<Color>) -> Self {
        SectionOptions {
            scale,
            font_stack_hash: font_stack_hash(&font_stack),
            font_stack,
            type_,
            shaped: None,
            key_section: true,
            rtl: false,
            image_id: None,
            text_color,
            vertical_align: VerticalAlign::Bottom,
        }
    }

    /// The options of an image section, which has no font stack.
    #[must_use]
    fn image(image_id: String, vertical_align: VerticalAlign) -> Self {
        SectionOptions {
            image_id: Some(image_id),
            vertical_align,
            font_stack_hash: 0,
            ..SectionOptions::new(1.0, FontStack::new(), GlyphIDType::FontPBF, None)
        }
    }
}

/// The characters that stand for inline images: the Private Use Area of the
/// Basic Multilingual Plane, which leaves room for 6400 images in a label.
const PUA_BEGIN: char = '\u{E000}';
const PUA_END: char = '\u{F8FF}';

/// A label's text, a character at a time, with each character's section, so
/// that operations that rearrange characters, such as bidi, keep their
/// formatting.
#[derive(Clone, Debug, Default)]
pub struct TaggedString {
    styled_text: StyledText,
    sections: Vec<SectionOptions>,
    /// The character of the last image section, if any.
    image_section_id: Option<char>,
    /// The text that font faces shaped this text from, once they have.
    unshaped_text: Option<Vec<char>>,
}

impl TaggedString {
    /// `text` in one section.
    #[must_use]
    pub fn new(text: Vec<char>, options: SectionOptions) -> Self {
        let styles = vec![0; text.len()];
        TaggedString { styled_text: StyledText { text, styles }, sections: vec![options], ..Default::default() }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.styled_text.text.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.styled_text.text.is_empty()
    }

    #[must_use]
    pub fn text(&self) -> &[char] {
        &self.styled_text.text
    }

    #[must_use]
    pub fn styled_text(&self) -> &StyledText {
        &self.styled_text
    }

    #[must_use]
    pub fn sections(&self) -> &[SectionOptions] {
        &self.sections
    }

    /// The index of the section of the character at `index`. Panics when
    /// out of range.
    #[must_use]
    pub fn section_index(&self, index: usize) -> SectionIndex {
        self.styled_text.styles[index]
    }

    /// The section of the character at `index`. Panics when out of range.
    #[must_use]
    pub fn section_of(&self, index: usize) -> &SectionOptions {
        &self.sections[usize::from(self.section_index(index))]
    }

    /// Appends `text` as a section with `options`.
    pub fn add_text_section(&mut self, text: &[char], options: SectionOptions) {
        debug_assert!(options.shaped.is_none() && options.image_id.is_none());
        self.styled_text.text.extend_from_slice(text);
        self.push_section(options);
    }

    /// Appends a section of glyphs that a font face shaped, in visual order,
    /// from text that runs right to left if `rtl`.
    pub fn add_shaped_section(&mut self, shaped: &Arc<ShapedText>, rtl: bool, options: SectionOptions) {
        debug_assert!(options.image_id.is_none());
        let len = self.styled_text.text.len() + shaped.glyphs.len();
        self.styled_text.text.resize(len, SHAPED_GLYPH);
        self.push_section(SectionOptions { shaped: Some(Arc::clone(shaped)), rtl, ..options });
    }

    /// Appends an inline image. A label holds at most 6400; it logs a
    /// warning and drops any more.
    pub fn add_image_section(&mut self, image_id: &str, vertical_align: VerticalAlign) {
        let Some(next) = self.next_image_char() else {
            log::warn!(target: log_target::STYLE, "Exceeded maximum number of images in a label.");
            return;
        };
        self.styled_text.text.push(next);
        self.push_section(SectionOptions::image(image_id.to_string(), vertical_align));
    }

    /// Adds `options` as the section of the characters that have none yet.
    fn push_section(&mut self, options: SectionOptions) {
        let index = self.sections.len() as SectionIndex;
        self.sections.push(options);
        self.styled_text.styles.resize(self.styled_text.text.len(), index);
    }

    /// Replaces punctuation drawn from glyph PBFs with its vertical forms,
    /// where its neighbors allow.
    pub fn verticalize_punctuation(&mut self) {
        let vertical = i18n::verticalize_punctuation_str(&self.styled_text.text);
        let StyledText { text, styles } = &mut self.styled_text;
        for ((c, vertical), &style) in text.iter_mut().zip(vertical).zip(styles.iter()) {
            if self.sections[usize::from(style)].type_ == GlyphIDType::FontPBF {
                *c = vertical;
            }
        }
    }

    /// Whether a character drawn from glyph PBFs stays upright in vertical
    /// writing mode.
    #[must_use]
    pub fn allows_vertical_writing_mode(&self) -> bool {
        self.styled_text.text.iter().zip(&self.styled_text.styles).any(|(&c, &style)| {
            self.sections[usize::from(style)].type_ == GlyphIDType::FontPBF && i18n::has_upright_vertical_orientation(c)
        })
    }

    /// Whether font faces shaped the text: [`Self::set_unshaped_text`].
    #[must_use]
    pub fn is_shaped(&self) -> bool {
        self.unshaped_text.is_some()
    }

    /// Marks the text as shaped by font faces from `text`, which the shaped
    /// sections no longer hold.
    pub fn set_unshaped_text(&mut self, text: Vec<char>) {
        self.unshaped_text = Some(text);
    }

    /// What the text says, as characters: the text before font faces
    /// shaped it, if they did. Labels with equal text compare equal.
    #[must_use]
    pub fn logical_text(&self) -> &[char] {
        self.unshaped_text.as_deref().unwrap_or(&self.styled_text.text)
    }

    /// Whether a section draws from a font file, which shapes it, rather
    /// than glyph PBFs.
    #[must_use]
    pub fn has_font_sections(&self) -> bool {
        self.sections.iter().any(|section| section.type_ != GlyphIDType::FontPBF)
    }

    fn next_image_char(&mut self) -> Option<char> {
        let next = match self.image_section_id {
            None => PUA_BEGIN,
            Some(last) => char::from_u32(u32::from(last) + 1).filter(|&next| next <= PUA_END)?,
        };
        self.image_section_id = Some(next);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // gtest: TaggedString.ImageSections
    #[test]
    fn image_sections() {
        let mut string = TaggedString::default();
        string.add_image_section("image_name", VerticalAlign::Bottom);
        assert_eq!(string.text(), ['\u{E000}']);
        assert_eq!(string.section_of(0).image_id.as_deref(), Some("image_name"));

        let mut max_sections = TaggedString::default();
        for i in 0..6401 {
            max_sections.add_image_section(&i.to_string(), VerticalAlign::Bottom);
        }

        assert_eq!(max_sections.sections().len(), 6400);
        assert_eq!(max_sections.text()[0], '\u{E000}');
        assert_eq!(max_sections.text()[6399], '\u{F8FF}');
    }

    #[test]
    fn verticalizes_only_glyph_pbf_punctuation() {
        let face = crate::text::glyph::face_type("tagged-string-test", "font", &[(0, 0xFFFF)]);
        let mut string = TaggedString::default();
        string.add_text_section(&['-'], SectionOptions::new(1.0, vec![], GlyphIDType::FontPBF, None));
        string.add_text_section(&['-'], SectionOptions::new(1.0, vec![], face, None));
        string.verticalize_punctuation();
        assert_eq!(string.text(), ['\u{FE32}', '-']);
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
