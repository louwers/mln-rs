//! A font face that the map shapes text with and draws glyphs from.

use std::sync::{Arc, OnceLock};

use skrifa::{Font, GlyphId, HintOptions, ScalerFont};

use crate::text::glyph::{FontData, Glyph, GlyphID};

/// The size glyphs are drawn and shaped at, in pixels per em.
pub(crate) const SDF_FONT_SIZE: i32 = 24;

#[derive(Clone)]
pub(crate) struct Face {
    /// The font at its weight (`weight::REGULAR_WEIGHT`, unless
    /// made at another weight), or `None` when the data
    /// holds no font this can read or the font maps no characters.
    font: Option<Font>,
    /// The hinted glyph loader at `SDF_FONT_SIZE`, made on first use.
    scaler: Arc<OnceLock<ScalerFont>>,
}

impl Face {
    /// Opens a face on `font_data`: an SFNT font or collection, or a WOFF or
    /// WOFF2 file. Its character map is a Unicode, symbol or Mac Roman
    /// `cmap` subtable; a font with none of those, such as a bare CFF font,
    /// isn't valid.
    ///
    /// The face reads `font_data` instead of copying it, unless it is a WOFF
    /// or WOFF2 file: then it reads the font decoded from it, and isn't valid
    /// when that fails. A variable font is at the OpenType `weight` within
    /// its `wght` axis.
    pub(crate) fn new(font_data: impl Into<FontData>, weight: f32) -> Self {
        let font = crate::text::woff::to_sfnt(font_data.into())
            .ok()
            .and_then(|data| Font::new(data.shared(), data.face_index()))
            .filter(|font| font.charmap().has_unicode())
            .map(|font| crate::text::weight::at_weight(font, weight));
        Face { font, scaler: Arc::default() }
    }

    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        self.font.is_some()
    }

    /// The font at the face's weight, `None` when not valid.
    #[must_use]
    pub(crate) fn font(&self) -> Option<&Font> {
        self.font.as_ref()
    }

    /// The hinted glyph loader of a valid face: the font's own hinting, or
    /// the autohinter for a font that has none.
    pub(crate) fn scaler(&self) -> Option<&ScalerFont> {
        let font = self.font.as_ref()?;
        Some(
            self.scaler
                .get_or_init(|| ScalerFont::hinted(font.clone(), SDF_FONT_SIZE as f32, HintOptions::auto_fallback())),
        )
    }

    /// The index of the glyph the font maps `c` to, if the map can draw it:
    /// a glyph other than `.notdef`, whose index fits a glyph ID and whose
    /// outline the map reads.
    pub(crate) fn glyph_of(&self, c: char) -> Option<u16> {
        let glyph = self.font()?.charmap().map_unicode(c)?;
        let index = u16::try_from(glyph.to_u32()).ok().filter(|&index| index != 0)?;
        self.scaler()?.outline(GlyphId::new(u32::from(index)))?;
        Some(index)
    }

    #[must_use]
    pub(crate) fn rasterize_glyph(&self, glyph: GlyphID) -> Glyph {
        crate::text::rasterize::rasterize_glyph(self, glyph)
    }
}

/// A font file, opened to find which characters the map can draw from it.
#[derive(Clone)]
pub struct FontFile {
    face: Face,
}

impl FontFile {
    /// The font in `data`, as a font face reads it: `None` when it isn't a
    /// font the map can draw.
    #[must_use]
    pub fn new(data: FontData) -> Option<Self> {
        // Which characters a font draws doesn't depend on its weight.
        let face = Face::new(data, crate::text::weight::REGULAR_WEIGHT);
        face.is_valid().then_some(FontFile { face })
    }

    /// Whether the map can draw `c` from the font: the font maps it to a
    /// glyph whose outline the map reads. A font whose outlines are in a
    /// format the map doesn't read has none.
    #[must_use]
    pub fn has_char(&self, c: char) -> bool {
        self.face.glyph_of(c).is_some()
    }
}

impl std::fmt::Debug for FontFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFile").finish_non_exhaustive()
    }
}
