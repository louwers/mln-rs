// Fonts without a Unicode `cmap` subtable. The fixtures are Noto Sans and
// Source Code Pro subset to "Hag" (`fonttools subset --text=Hag
// --name-IDs='*'`): Noto Sans with its `cmap` holding the same mappings in
// one Mac Roman (1, 0) subtable instead, and Source Code Pro's `CFF ` table
// on its own, a bare CFF font, which has no `cmap`.

use crate::glyph::GlyphIDType;
use crate::{FreeTypeLibrary, HBShaper};

const MAC_ROMAN: &[u8] = include_bytes!("../../tests/fixtures/mac-roman.ttf");
const BARE_CFF: &[u8] = include_bytes!("../../tests/fixtures/bare.cff");

/// The number of glyphs of `text` that `font` maps and draws with ink, or
/// `None` when it isn't a font the shaper reads.
fn inked_glyphs(font: &[u8], text: &str) -> Option<usize> {
    let shaper = HBShaper::new(GlyphIDType(1), font.to_vec(), &FreeTypeLibrary::new());
    if !shaper.valid() {
        return None;
    }
    let text: Vec<u16> = text.encode_utf16().collect();
    let (mut ids, mut adjusts) = (Vec::new(), Vec::new());
    shaper.create_complex_glyph_ids(&text, false, &mut ids, &mut adjusts);
    Some(ids.iter().filter(|id| id.code != 0 && shaper.rasterize_glyph(id).bitmap.data.iter().any(|&a| a > 0)).count())
}

#[test]
fn a_mac_roman_character_map_maps_text() {
    assert_eq!(inked_glyphs(MAC_ROMAN, "Hag"), Some(3));
}

/// A font that maps no characters isn't valid, so that the map draws its
/// text with another font rather than with `.notdef` glyphs.
#[test]
fn a_font_without_a_character_map_is_not_valid() {
    assert_eq!(inked_glyphs(BARE_CFF, "Hag"), None);
}
