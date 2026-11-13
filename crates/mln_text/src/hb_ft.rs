// HarfRust `FontFuncs` over the FreeType face: the font functions of
// HarfBuzz 8.2.0's hb-ft.cc that HarfBuzz calls while shaping horizontal
// text (`hb_ft_get_nominal_glyph(s)`, `hb_ft_get_variation_glyph`,
// `hb_ft_get_glyph_h_advances`, plus `hb_ft_font_changed`'s scale). Under
// HarfBuzz's "Old MIT" license (LICENSE-HarfBuzz).
//
// hb-ft also registers extents, contour-point, kerning and vertical
// callbacks; the C++ core's HarfBuzz is built with `HB_TINY`, whose shaping
// never calls them for horizontal text (no fallback mark positioning or
// kerning, no hinting), so they are left out here. `glyph_extents` returns
// `None`, which keeps HarfRust's fallback mark positioning from moving marks
// (see `harfbuzz_impl.rs`).

use harfrust::{FontFuncs, GlyphExtents, GlyphId, ShaperFont};

use crate::freetype::FreeTypeFace;
use crate::ft::cmap::Encoding;

/// `hb_ft_font_t` for one face.
pub(crate) struct HbFtFont<'a> {
    face: &'a FreeTypeFace,
    /// Whether the selected charmap is a symbol charmap.
    symbol: bool,
    font_page: u16,
    /// -1 when the font's x scale is negative, +1 otherwise.
    x_mult: f32,
}

impl<'a> HbFtFont<'a> {
    /// `_hb_ft_font_create` (`hb_ft_font_create` without the scale part).
    #[must_use]
    pub(crate) fn new(face: &'a FreeTypeFace) -> Self {
        let ft = face.get_face().expect("hb-ft needs a valid face");
        let symbol = ft.charmap.is_some_and(|i| ft.charmaps[i].encoding == Encoding::MsSymbol);
        // OS/2 `get_font_page`: `version == 0 ? fsSelection & 0xFF00 : 0`;
        // HarfBuzz reads a missing or broken table as zeros.
        let font_page = if ft.os2_version == 0 { ft.os2_fs_selection & 0xFF00 } else { 0 };
        let (x_scale, _) = Self::scale(face);
        HbFtFont { face, symbol, font_page, x_mult: if x_scale < 0 { -1.0 } else { 1.0 } }
    }

    /// `hb_ft_font_changed`: `hb_font_set_scale` from the FreeType size,
    /// `(x_scale * units_per_EM + 2^15) >> 16` in unsigned 64-bit.
    #[must_use]
    pub(crate) fn scale(face: &FreeTypeFace) -> (i32, i32) {
        let ft = face.get_face().expect("hb-ft needs a valid face");
        let upem = u64::from(ft.units_per_em);
        let x = ((ft.x_scale as u64).wrapping_mul(upem).wrapping_add(1 << 15) >> 16) as i32;
        let y = ((ft.y_scale as u64).wrapping_mul(upem).wrapping_add(1 << 15) >> 16) as i32;
        (x, y)
    }
}

impl FontFuncs for HbFtFont<'_> {
    /// `hb_ft_get_nominal_glyph`.
    // The `match` is HarfBuzz's `switch` over the font page.
    #[allow(clippy::single_match, clippy::collapsible_match)]
    fn nominal_glyph(&self, _font: &ShaperFont, unicode: u32) -> Option<GlyphId> {
        let mut g = self.face.get_char_index(unicode);
        if g == 0 {
            if self.symbol {
                match self.font_page {
                    // FONT_PAGE_NONE: for symbol-encoded OpenType fonts, the
                    // U+F000..F0FF range is duplicated at U+0000..U+00FF.
                    0 => {
                        if unicode <= 0x00FF {
                            g = self.face.get_char_index(0xF000 + unicode);
                        }
                    }
                    // The Arabic PUA pages are HB_NO_OT_SHAPER_ARABIC_FALLBACK
                    // in HB_TINY.
                    _ => {}
                }
                if g == 0 {
                    return None;
                }
            } else {
                return None;
            }
        }
        Some(GlyphId::new(g))
    }

    // `hb_ft_get_nominal_glyphs` stops at the first unmapped codepoint and
    // HarfBuzz retries that one with the singular callback, so the trait's
    // default batch (singular per codepoint, stopping at the first failure)
    // gives the same glyphs.

    /// `hb_ft_get_variation_glyph`.
    fn variation_glyph(&self, _font: &ShaperFont, unicode: u32, variation_selector: u32) -> Option<GlyphId> {
        let g = self.face.get_char_variant_index(unicode, variation_selector);
        if g == 0 { None } else { Some(GlyphId::new(g)) }
    }

    /// `hb_ft_get_glyph_h_advances` for one glyph.
    fn glyph_h_advance(&self, _font: &ShaperFont, glyph: GlyphId) -> i32 {
        // FT_Fixed v = 0; FT_Get_Advance (ft_face, glyph, load_flags, &v);
        let v = self.face.get_advance(glyph.to_u32()).unwrap_or(0);
        // Work around bug that FreeType seems to return negative advance
        // for variable-set fonts if x_scale is negative!
        let v = v.abs();
        // v = (int) (v * x_mult + (1<<9)) >> 10;  (float arithmetic)
        ((v as f32 * self.x_mult + (1 << 9) as f32) as i32) >> 10
    }

    /// Not used by `HB_TINY` shaping (see the file comment).
    fn glyph_extents(&self, _font: &ShaperFont, _glyph: GlyphId) -> Option<GlyphExtents> {
        None
    }
}
