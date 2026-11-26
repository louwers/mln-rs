// The C++ shapes with HarfBuzz 8.2.0 built with HB_TINY on an hb-ft font
// (`hb_ft_font_create`). The port shapes with HarfRust on the face's font,
// with its own character map and advances, at 24 px in 26.6 (hb-ft's
// scale), and:
// - the font's tables, including the AAT ones (morx, kerx, trak and the
//   rest) and `kern`, which HB_TINY compiles out: some system fonts, like
//   Apple's Tibetan font Kailasa, shape only through `morx`;
// - user features `frac=0`, `numr=0`, `dnom=0` (HB_NO_OT_SHAPE_FRACTIONS:
//   HB_TINY never adds the automatic fraction features);
// - no glyph extents, as hb-ft under HB_TINY gives none, which keeps
//   HarfRust's fallback mark positioning from moving marks.
// Unlike HB_TINY's HB_NO_VAR, a variable font shapes at its weight (regular
// unless made `Shaper::at_weight`), with its GSUB/GPOS feature variations.
// HarfRust has no switch for some HB_TINY options (HB_NO_OT_SHAPE_FALLBACK,
// HB_NO_OT_SHAPER_VOWEL_CONSTRAINTS, HB_NO_UCD_UNASSIGNED), so a few texts
// shape differently: fallback mark positioning, an inserted U+25CC, and
// Unicode 18.0 instead of 15.0 properties.

use std::sync::Arc;

use harfrust::{
    Buffer, Direction, Feature, FontFuncs, GlyphExtents, GlyphId, Language, Script, ShapeOptions, ShapePlan,
    ShaperFont, Tag,
};
use mln_style::hash::HashMap;
use parking_lot::Mutex;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::face::{Face, SDF_FONT_SIZE};
use crate::glyph::{FontData, Glyph, GlyphID, GlyphIDType};

/// Where a shaped glyph goes, in pixels at 24 px: its offset from the pen,
/// and how far it moves the pen.
#[derive(Clone, Copy, PartialEq, Debug, Default, mln_wire::Wire)]
pub struct GlyphAdjust {
    pub x_offset: f32,
    pub y_offset: f32,
    pub advance: f32,
}

impl GlyphAdjust {
    #[must_use]
    pub fn new(x_offset: f32, y_offset: f32, advance: f32) -> Self {
        GlyphAdjust { x_offset, y_offset, advance }
    }
}

/// Shaped text: its glyphs in visual order, and where each goes.
#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct ShapedText {
    pub glyphs: Vec<GlyphID>,
    pub adjusts: Vec<GlyphAdjust>,
}

/// HarfBuzz's default language, which with `HB_NO_SETLOCALE` is "c".
fn get_default_language() -> Language {
    Language::new("c").expect("non-empty")
}

/// The script of `c`, as an ISO 15924 tag. Combining marks inherit the
/// script of their bases, regardless of their own script. Uses the Unicode
/// version of HarfRust's own tables.
#[must_use]
pub(crate) fn get_unicode_script(c: char) -> [u8; 4] {
    if c.general_category() == GeneralCategory::NonspacingMark {
        return Script::INHERITED.tag().to_be_bytes();
    }
    harfrust::unicode::script(u32::from(c)).tag().to_be_bytes()
}

/// Shape plans by script tag and right-to-left direction.
type ShapePlans = HashMap<([u8; 4], bool), Arc<ShapePlan>>;

/// A font file that shapes text with HarfRust and draws its glyphs.
pub struct Shaper {
    type_: GlyphIDType,
    face: Face,
    /// HarfBuzz caches shape plans per segment properties (script and
    /// direction); so do we.
    plans: Mutex<ShapePlans>,
}

/// The user features that stand in for `HB_NO_OT_SHAPE_FRACTIONS`.
fn hb_tiny_features() -> [Feature; 3] {
    [
        Feature::new(Tag::new(b"frac"), 0, ..),
        Feature::new(Tag::new(b"numr"), 0, ..),
        Feature::new(Tag::new(b"dnom"), 0, ..),
    ]
}

/// The font's own glyph mapping and advances, without glyph extents.
struct NoGlyphExtents;

impl FontFuncs for NoGlyphExtents {
    fn glyph_extents(&self, _font: &ShaperFont, _glyph: GlyphId) -> Option<GlyphExtents> {
        None
    }
}

impl Shaper {
    /// A shaper for the font in `data`, whose glyphs have type `type_`. It
    /// keeps `data` instead of copying it.
    pub fn new(type_: GlyphIDType, data: impl Into<FontData>) -> Self {
        Shaper::at_weight(type_, data, crate::weight::REGULAR_WEIGHT)
    }

    /// [`Shaper::new`], with a variable font at the OpenType `weight` within
    /// its `wght` axis rather than at regular weight.
    pub fn at_weight(type_: GlyphIDType, data: impl Into<FontData>, weight: f32) -> Self {
        Shaper { type_, face: Face::new(data, weight), plans: Mutex::new(HashMap::default()) }
    }

    /// Whether the data is a font the shaper reads.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.face.is_valid()
    }

    /// The glyph the font maps `c` to, unshaped: `None` unless it has one
    /// that the shaper draws, a glyph other than `.notdef` with an outline
    /// it reads.
    #[must_use]
    pub fn glyph_of(&self, c: char) -> Option<GlyphID> {
        Some(GlyphID::with_type(self.face.glyph_of(c)?, self.type_))
    }

    /// The glyph's coverage at 24 px, before the signed distance field.
    #[must_use]
    pub fn rasterize_glyph(&self, glyph: GlyphID) -> Glyph {
        self.face.rasterize_glyph(glyph)
    }

    fn plan(
        &self,
        font: &harfrust::Font,
        script: [u8; 4],
        rtl: bool,
        language: &Language,
        features: &[Feature],
    ) -> Arc<ShapePlan> {
        let mut plans = self.plans.lock();
        Arc::clone(plans.entry((script, rtl)).or_insert_with(|| {
            Arc::new(ShapePlan::new(font, direction(rtl), Some(to_script(script)), Some(language), features))
        }))
    }

    /// Appends the glyphs of `text`, one run of one direction in logical
    /// order, and where each goes. Glyphs come back in visual order, so a
    /// right-to-left run's glyphs come back reversed.
    pub fn shape(&self, text: &[char], rtl: bool, shaped: &mut ShapedText) {
        struct TextPart {
            text: Vec<char>,
            script: [u8; 4],
        }

        // HB_SCRIPT_INVALID, for a part that holds no letters yet.
        const INVALID: [u8; 4] = [0; 4];

        if text.is_empty() {
            return;
        }
        // An invalid font shapes nothing.
        let Some(font) = self.face.font() else {
            return;
        };

        let is_neutral = |script: &[u8; 4]| matches!(script, b"Zyyy" | b"Zinh" | b"Zzzz");

        let mut text_parts: Vec<TextPart> = Vec::new();
        if rtl {
            // A right-to-left run may hold more than one script, such as
            // Hebrew and Arabic. Each script is shaped on its own, so the
            // font applies that script's rules, such as Arabic joining.
            // Spaces, punctuation, and marks belong to the letters before
            // them, or to the first letters.
            for &ch in text {
                let script = get_unicode_script(ch);
                let part_script = if is_neutral(&script) { INVALID } else { script };
                match text_parts.last_mut() {
                    Some(last) if part_script == INVALID || last.script == script || last.script == INVALID => {
                        if last.script == INVALID {
                            last.script = part_script;
                        }
                        last.text.push(ch);
                    }
                    _ => text_parts.push(TextPart { text: vec![ch], script: part_script }),
                }
            }
            for text_part in &mut text_parts {
                if text_part.script == INVALID {
                    text_part.script = *b"Arab";
                }
            }
            // HarfBuzz returns each part's glyphs in visual order, and the
            // run's later parts lie to the left of its earlier ones.
            text_parts.reverse();
        } else {
            for &ch in text {
                let script = get_unicode_script(ch);
                match text_parts.last_mut() {
                    Some(last) if last.script == script || script == *b"Zinh" => last.text.push(ch),
                    _ => text_parts.push(TextPart { text: vec![ch], script }),
                }
            }
        }

        for text_part in &text_parts {
            let Some(buffer) = self.shape_part(font, &text_part.text, text_part.script, rtl) else {
                continue;
            };

            let glyph_info = buffer.glyph_infos();
            let glyph_pos = buffer.glyph_positions();
            shaped.glyphs.reserve(glyph_info.len());
            shaped.adjusts.reserve(glyph_info.len());
            for (info, pos) in glyph_info.iter().zip(glyph_pos) {
                shaped.glyphs.push(GlyphID::with_type(info.glyph_id as u16, self.type_));
                shaped.adjusts.push(GlyphAdjust {
                    x_offset: pos.x_offset as f32 / 64.0,
                    y_offset: pos.y_offset.wrapping_neg() as f32 / 64.0,
                    advance: pos.x_advance as f32 / 64.0,
                });
            }
        }
    }

    /// Shapes one part of a run, of one script.
    fn shape_part(&self, font: &harfrust::Font, text: &[char], script: [u8; 4], rtl: bool) -> Option<Buffer> {
        let shaper_font = ShaperFont::new(font).with_scale(SDF_FONT_SIZE * 64).with_font_funcs(Some(&NoGlyphExtents));
        let language = get_default_language();
        let features = hb_tiny_features();

        let mut buffer = Buffer::new();
        buffer.set_direction(direction(rtl));
        buffer.set_script(Some(to_script(script)));
        buffer.set_language(Some(language.clone()));
        // Each character is a cluster of its own, with no pre- or
        // post-context.
        for (cluster, &c) in (0..).zip(text) {
            buffer.push(u32::from(c), cluster);
        }

        let plan = self.plan(font, script, rtl, &language, &features);
        let options = ShapeOptions::new().plan(Some(&plan)).features(&features);
        // hb_shape cannot fail on a valid buffer; HarfRust only fails on
        // plan/buffer mismatches, which cannot happen here.
        harfrust::shape(&shaper_font, &mut buffer, options).ok()?;
        Some(buffer)
    }
}

/// `rtl ? HB_DIRECTION_RTL : HB_DIRECTION_LTR`.
fn direction(rtl: bool) -> Direction {
    if rtl { Direction::RightToLeft } else { Direction::LeftToRight }
}

/// `hb_script_t` (a tag) as a HarfRust script.
fn to_script(tag: [u8; 4]) -> Script {
    Script::from_iso15924_tag(Tag::new(&tag)).unwrap_or(Script::UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_segmentation_classifies_characters() {
        assert_eq!(&get_unicode_script('a'), b"Latn");
        assert_eq!(&get_unicode_script(' '), b"Zyyy");
        assert_eq!(&get_unicode_script('\u{0301}'), b"Zinh");
        assert_eq!(&get_unicode_script('\u{0941}'), b"Zinh"); // Devanagari vowel sign U (Mn)
        assert_eq!(&get_unicode_script('\u{0627}'), b"Arab");
        assert_eq!(&get_unicode_script('\u{4E2D}'), b"Hani");
        assert_eq!(&get_unicode_script('\u{AC00}'), b"Hang");
        assert_eq!(&get_unicode_script('\u{20000}'), b"Hani"); // CJK Extension B
        assert_eq!(&get_unicode_script('\u{1F600}'), b"Zyyy");
    }
}
