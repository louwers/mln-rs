// The faces of a font collection. The fixture holds the Khmer fixture
// (woff.rs) and Noto Sans CJK JP subset to the glyph of "中" at weight 400
// (`fonttools subset NotoSansCJK-VF.ttc --font-number=0 --text=中
// --layout-features='*' --name-IDs='*'`, `fonttools varLib.instancer
// wght=400`), collected by `fonttools ttLib -o collection.ttc`.

use crate::glyph::{FontData, GlyphIDType};
use crate::{FreeTypeLibrary, HBShaper};

const COLLECTION: &[u8] = include_bytes!("../../tests/fixtures/collection.ttc");

/// The number of glyphs of `text` that face `index` of the fixture draws
/// with ink, or `None` when it has no such face.
fn inked_glyphs(index: u32, text: &str) -> Option<usize> {
    let data = FontData::from(COLLECTION.to_vec()).with_face_index(index);
    let shaper = HBShaper::new(GlyphIDType(1), data, &FreeTypeLibrary::new());
    if !shaper.valid() {
        return None;
    }
    let text: Vec<u16> = text.encode_utf16().collect();
    let (mut ids, mut adjusts) = (Vec::new(), Vec::new());
    shaper.create_complex_glyph_ids(&text, false, &mut ids, &mut adjusts);
    Some(ids.iter().filter(|id| id.code != 0 && shaper.rasterize_glyph(id).bitmap.data.iter().any(|&a| a > 0)).count())
}

#[test]
fn each_face_of_a_collection_draws_its_own_glyphs() {
    assert_eq!(inked_glyphs(0, "中"), Some(0));
    assert!(inked_glyphs(0, "ខ").unwrap() > 0);
    assert_eq!(inked_glyphs(1, "中"), Some(1));
    assert_eq!(inked_glyphs(1, "ខ"), Some(0));
    assert_eq!(inked_glyphs(2, "中"), None);
}

#[test]
fn a_single_font_has_only_face_zero() {
    let khmer = include_bytes!("../../tests/fixtures/khmer.ttf").to_vec();
    let face = |index| {
        HBShaper::new(GlyphIDType(1), FontData::from(khmer.clone()).with_face_index(index), &FreeTypeLibrary::new())
    };
    assert!(face(0).valid());
    assert!(!face(1).valid());
}
