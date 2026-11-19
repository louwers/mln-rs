use std::sync::Arc;

use mln_text::glyph::{FontData, FontFace, Glyph, GlyphID};
use mln_text::local_glyph_rasterizer::{LocalFont, LocalGlyphRasterizer, LocalGlyphs};
use mln_util::font_stack::FontStack;

use crate::support::MapTest;

const KHMER: &[u8] = include_bytes!("../../mln_text/tests/fixtures/khmer.ttf");

/// The device's fonts as one, the Khmer font of `mln_text`'s fixtures, for
/// Khmer.
struct KhmerFont;

impl LocalGlyphRasterizer for KhmerFont {
    fn rasterize_glyph(&self, _font_stack: &FontStack, _glyph_id: GlyphID) -> Option<Glyph> {
        None
    }

    fn font(&self, _font_stack: &FontStack, c: char) -> Option<LocalFont> {
        ('\u{1780}'..='\u{17FF}').contains(&c).then(|| LocalFont {
            key: Arc::from("khmer"),
            data: FontData::from(KHMER.to_vec()),
            weight: 400,
        })
    }
}

/// How many pixels a map of Khmer text without glyph PBFs draws: with the
/// device's fonts as a fallback face if `system`.
fn ink(system: bool) -> usize {
    let test = MapTest::new();
    test.frontend().set_local_glyphs(LocalGlyphs { ideographs: false, rasterizer: Some(Arc::new(KhmerFont)) });
    if system {
        test.map().set_fallback_font_face_list(vec![FontFace::system(Vec::new())]);
    }
    test.load_json(
        r#"{
        "version": 8,
        "sources": {"point": {"type": "geojson", "data": {
            "type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [0, 0]}}}},
        "layers": [{"id": "symbol", "type": "symbol", "source": "point",
                    "layout": {"text-field": "\u1780\u17D2\u1780", "text-font": ["Any Font"], "text-size": 48}}]
    }"#,
    );
    let image = test.try_render().expect("a render").image;
    image.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] != 0).count()
}

// A style without glyph PBFs draws its text from the device's fonts, the
// ones the labels' `text-font` names first, shaped, as the style spec
// resolves its fonts; else each character from local glyphs, which this
// device draws none of.
#[test]
fn a_style_without_glyph_pbfs_shapes_its_text_with_the_devices_fonts() {
    assert_eq!(ink(false), 0);
    assert!(ink(true) > 0);
}
