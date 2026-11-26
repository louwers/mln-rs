//! Text drawn with the fonts on the device rather than the style's.
//!
//! Text that the device's fonts shape ([`FontFace::system`]) is shaped a
//! piece at a time: each piece the clusters, a character and the marks and
//! joiners after it, that the [`FontChooser`] chose one face for.
//!
//! A character that glyph PBFs could draw takes its glyph from local fonts,
//! one at a time, when the style has no `glyphs` URL and the device's fonts
//! don't shape it, when the stack's PBF lacks it, or, if
//! [`LocalGlyphs::ideographs`] is set, first when it is a CJK ideograph,
//! kana or hangul. Local glyphs come from the style's fallback font faces
//! first, then the face the [`FontChooser`] chooses, then the
//! [`LocalGlyphRasterizer`].
//!
//! [`FontFace::system`]: crate::text::glyph::FontFace::system

use std::sync::Arc;

use mln_util::font_stack::FontStack;

use crate::text::glyph::{FontFace, Glyph, GlyphID};

/// Chooses which of the device's fonts draws a cluster: a character and the
/// marks and joiners after it ([`i18n::extends_previous`]). A choice may
/// block on the platform's font matcher and read font files, so the glyph
/// store calls it on its thread pool, never on the render thread.
///
/// [`i18n::extends_previous`]: mln_style::i18n::extends_previous
pub trait FontChooser: Send + Sync {
    /// The device's font that draws `cluster` in a label of `font_stack`: the
    /// first font the stack names that the device has and that draws all of
    /// it, else the font the platform picks for it at the weight and style of
    /// the stack's first name ([`font_name`](crate::text::font_name)), else the
    /// font of its first character alone. `None` when no font draws that.
    ///
    /// The face is one [`FontFace::device`] made: it holds its font file, and
    /// the same font at the same weight always has the same `type_`.
    fn choose(&self, font_stack: &FontStack, cluster: &[char]) -> Option<FontFace>;
}

/// Draws a character's glyph at once, without a font file, as the browser's
/// canvas does. It doesn't shape.
pub trait LocalGlyphRasterizer: Send + Sync {
    /// The coverage of the character `glyph_id` names, for a label in
    /// `font_stack`, at 24 px, which the glyph store turns into a signed
    /// distance field. A bitmap twice that size sets
    /// `GlyphMetrics::is_double_resolution`, with the metrics of 24 px.
    /// `None` when the device can't draw it.
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
    /// The device's fonts, after the style's fallback faces: they shape
    /// [`FontFace::system`] text and draw local glyphs.
    pub chooser: Option<Arc<dyn FontChooser>>,
    /// The last source of local glyphs, for a platform without font files
    /// (the browser).
    pub rasterizer: Option<Arc<dyn LocalGlyphRasterizer>>,
}

impl std::fmt::Debug for LocalGlyphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalGlyphs")
            .field("ideographs", &self.ideographs)
            .field("chooser", &self.chooser.is_some())
            .field("rasterizer", &self.rasterizer.is_some())
            .finish()
    }
}
