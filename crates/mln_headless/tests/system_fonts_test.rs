use std::sync::Arc;

use mln_style::Edit;
use mln_text::glyph::{FontData, FontFace, Glyph, GlyphID};
use mln_text::local_glyph_rasterizer::{LocalFont, LocalGlyphRasterizer, LocalGlyphs};
use mln_util::font_stack::FontStack;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::gtest_map::style_image;
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

/// The pixels of a map without glyph PBFs that draws `text_field`, with the
/// device's fonts as a fallback face if `system`, and a red icon "icon".
fn render(system: bool, text_field: &str) -> Vec<[u8; 4]> {
    let test = MapTest::new();
    test.frontend().set_local_glyphs(LocalGlyphs { ideographs: false, rasterizer: Some(Arc::new(KhmerFont)) });
    if system {
        test.map().set_fallback_font_face_list(vec![FontFace::system(Vec::new())]);
    }
    test.load_json(&format!(
        r#"{{
        "version": 8,
        "sources": {{"point": {{"type": "geojson", "data": {{
            "type": "Feature", "properties": {{}}, "geometry": {{"type": "Point", "coordinates": [0, 0]}}}}}}}},
        "layers": [{{"id": "symbol", "type": "symbol", "source": "point",
                    "layout": {{"text-field": {text_field}, "text-font": ["Any Font"], "text-size": 48}}}}]
    }}"#
    ));
    let icon = PremultipliedImage::from_vec(Size::new(16, 16), [255, 0, 0, 255].repeat(16 * 16));
    test.map().edit_style(|style| style.apply(Edit::AddImage(style_image("icon", icon, 1.0)))).unwrap();
    let image = test.try_render().expect("a render").image;
    image.data.as_chunks::<4>().0.iter().map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]]).collect()
}

const KHMER_TEXT: &str = r#""\u1780\u17D2\u1780""#;

// A style without glyph PBFs draws its text from the device's fonts, the
// ones the labels' `text-font` names first, shaped, as the style spec
// resolves its fonts; else each character from local glyphs, which this
// device draws none of.
#[test]
fn a_style_without_glyph_pbfs_shapes_its_text_with_the_devices_fonts() {
    let ink = |pixels: Vec<[u8; 4]>| pixels.iter().filter(|pixel| pixel[3] != 0).count();
    assert_eq!(ink(render(false, KHMER_TEXT)), 0);
    assert!(ink(render(true, KHMER_TEXT)) > 0);
}

#[test]
fn an_image_beside_shaped_text_stays_an_image() {
    let field = format!(r#"["format", {KHMER_TEXT}, {{}}, ["image", "icon"], {{}}]"#);
    let red = render(true, &field).iter().filter(|pixel| pixel[0] > 200 && pixel[1] < 50 && pixel[3] > 200).count();
    assert!(red > 0);
}
