//! Text drawn with the fonts on the device rather than the style's.
//!
//! Text that the device's fonts shape ([`FontFace::system`]) takes, for
//! each character, the [`LocalGlyphRasterizer::font`] that has it. A
//! character that glyph PBFs could draw takes its glyph from local fonts,
//! one at a time, when the style has no `glyphs` URL and the device's fonts
//! don't shape it, when the stack's PBF lacks it, or, if
//! [`LocalGlyphs::ideographs`] is set, first when it is a CJK ideograph,
//! kana or hangul. The glyph store asks the style's fallback font faces
//! first, then the [`LocalGlyphRasterizer`].
//!
//! [`FontFace::system`]: crate::glyph::FontFace::system

use std::sync::Arc;

use mln_util::font_stack::FontStack;

use crate::glyph::{FontData, Glyph, GlyphID};

/// One of the device's font files, which shapes text: a face of it, at the
/// weight a font name asks for.
#[derive(Clone, Debug)]
pub struct LocalFont {
    /// Names the file, the face and the weight: equal for the same font.
    pub key: Arc<str>,
    pub data: FontData,
    /// The OpenType weight a variable font draws at.
    pub weight: u16,
}

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

    /// The font that shapes `c` for a label in `font_stack`, from the same
    /// fonts as `rasterize_glyph`. `None` when no local font has `c`, and
    /// from a rasterizer without font files, such as the browser's.
    fn font(&self, font_stack: &FontStack, c: char) -> Option<LocalFont> {
        let _ = (font_stack, c);
        None
    }
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
