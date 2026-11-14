//! A font face that the map shapes text with and draws glyphs from.

use std::sync::{Arc, OnceLock};

use skrifa::{Font, HintOptions, ScalerFont};

use crate::glyph::{FontData, Glyph, GlyphID};

/// The size glyphs are drawn and shaped at, in pixels per em.
pub(crate) const SDF_FONT_SIZE: i32 = 24;

/// A handle that [`HBShaper::new`](crate::HBShaper::new) takes. It holds no
/// state.
#[derive(Debug, Default)]
pub struct FreeTypeLibrary;

impl FreeTypeLibrary {
    #[must_use]
    pub fn new() -> Self {
        FreeTypeLibrary
    }
}

#[derive(Clone)]
pub(crate) struct Face {
    /// The font at regular weight (`weight::at_regular_weight`), or `None`
    /// when the data holds no font this can read or the font maps no
    /// characters.
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
    /// when that fails.
    pub(crate) fn from_memory(font_data: impl Into<FontData>) -> Self {
        let font = crate::woff::to_sfnt(font_data.into())
            .ok()
            .and_then(|data| Font::new(data.shared(), data.face_index()))
            .filter(|font| font.charmap().has_unicode())
            .map(crate::weight::at_regular_weight);
        Face { font, scaler: Arc::default() }
    }

    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        self.font.is_some()
    }

    /// The font at regular weight, `None` when not valid.
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

    #[must_use]
    pub(crate) fn rasterize_glyph(&self, glyph: GlyphID) -> Glyph {
        crate::rasterize::rasterize_glyph(self, glyph)
    }
}
