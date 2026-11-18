//! Glyphs drawn from the fonts on the device rather than from glyph PBFs.
//!
//! A character that glyph PBFs could draw takes its glyph from local fonts
//! when the style has no `glyphs` URL, when the stack's PBF lacks it, or, if
//! [`LocalGlyphs::ideographs`] is set, first when it is a CJK ideograph,
//! kana or hangul. The glyph store asks the style's fallback font faces
//! first, then the [`LocalGlyphRasterizer`].

use std::sync::Arc;

use mln_util::font_stack::FontStack;

use crate::glyph::{Glyph, GlyphID};

/// Draws the glyphs of characters from the device's fonts: the fonts that a
/// font stack names, if the device has them, else the font the platform
/// picks for the character, at the weight and style that the stack's names
/// imply ([`font_name`](crate::font_name)).
pub trait LocalGlyphRasterizer: Send + Sync {
    /// The glyph of the character `glyph_id` names, for a label in
    /// `font_stack`: its coverage at 24 px, which the glyph store turns into
    /// a signed distance field. A bitmap twice that size sets
    /// `GlyphMetrics::is_double_resolution`, with the metrics of 24 px.
    /// `None` when no local font has the character.
    fn rasterize_glyph(&self, font_stack: &FontStack, glyph_id: GlyphID) -> Option<Glyph>;
}

/// How a renderer draws glyphs from local fonts. The default draws none,
/// so that a map draws the same everywhere.
#[derive(Clone, Default)]
pub struct LocalGlyphs {
    /// Whether CJK ideographs, kana and hangul
    /// (`i18n::allows_fixed_width_glyph_generation`) come from local fonts
    /// before glyph PBFs, as MapLibre GL JS's `localIdeographFontFamily`
    /// draws them: a map with such labels downloads a glyph PBF for every
    /// few characters otherwise.
    pub ideographs: bool,
    /// The device's fonts, after the style's fallback font faces.
    pub rasterizer: Option<Arc<dyn LocalGlyphRasterizer>>,
}

impl std::fmt::Debug for LocalGlyphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalGlyphs")
            .field("ideographs", &self.ideographs)
            .field("rasterizer", &self.rasterizer.is_some())
            .finish()
    }
}
