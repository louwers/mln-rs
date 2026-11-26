use std::sync::{Arc, LazyLock};
use std::thread::ThreadId;

use mln_engine::text::font_stack::FontStack;
use mln_engine::text::glyph::{FontData, FontFace};
use mln_engine::text::local_fonts::{FontChooser, LocalGlyphs};
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;
use parking_lot::Mutex;

use crate::gtest_map::style_image;
use crate::support::MapTest;

static KHMER: LazyLock<FontData> =
    LazyLock::new(|| FontData::from(include_bytes!("../../mln_engine/tests/fixtures/khmer.ttf").to_vec()));

/// The device's fonts as one, the Khmer font of `mln_engine`'s fixtures, for
/// Khmer. Records the threads it chooses on.
#[derive(Default)]
struct KhmerFont {
    threads: Mutex<Vec<ThreadId>>,
}

impl FontChooser for KhmerFont {
    fn choose(&self, _font_stack: &FontStack, cluster: &[char]) -> Option<FontFace> {
        self.threads.lock().push(std::thread::current().id());
        let khmer = cluster.iter().all(|c| ('\u{1780}'..='\u{17FF}').contains(c));
        khmer.then(|| FontFace::device("khmer", 400, KHMER.clone()))
    }
}

/// The pixels of a map without glyph PBFs that draws `text_field`, with the
/// device's fonts as a fallback face if `system`, and a red icon "icon".
fn render(system: bool, text_field: &str) -> Vec<[u8; 4]> {
    let test = MapTest::new();
    let khmer = Arc::new(KhmerFont::default());
    let chooser: Arc<dyn FontChooser> = Arc::<KhmerFont>::clone(&khmer);
    test.frontend().set_local_glyphs(LocalGlyphs { ideographs: false, chooser: Some(chooser), rasterizer: None });
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
    // The frontend renders on this thread.
    let threads = khmer.threads.lock();
    assert!(!threads.is_empty());
    assert!(threads.iter().all(|thread| *thread != std::thread::current().id()), "chose on the render thread");
    image.data.as_chunks::<4>().0.iter().map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]]).collect()
}

/// "ខ្មែរ", which the fixture has the glyphs of: a subscript consonant and a
/// split vowel, which shaping rearranges and positions.
const KHMER_TEXT: &str = r#""\u1781\u17D2\u1798\u17C2\u179A""#;

// A style without glyph PBFs draws its text from the device's fonts, the
// ones the labels' `text-font` names first, shaped, as the style spec
// resolves its fonts; else each character from local glyphs, which come from
// the same fonts, one at a time, unshaped.
#[test]
fn a_style_without_glyph_pbfs_shapes_its_text_with_the_devices_fonts() {
    let ink = |pixels: &[[u8; 4]]| pixels.iter().filter(|pixel| pixel[3] != 0).count();
    let (unshaped, shaped) = (render(false, KHMER_TEXT), render(true, KHMER_TEXT));
    assert!(ink(&unshaped) > 0);
    assert!(ink(&shaped) > 0);
    assert_ne!(unshaped, shaped);
}

#[test]
fn an_image_beside_shaped_text_stays_an_image() {
    let field = format!(r#"["format", {KHMER_TEXT}, {{}}, ["image", "icon"], {{}}]"#);
    let red = render(true, &field).iter().filter(|pixel| pixel[0] > 200 && pixel[1] < 50 && pixel[3] > 200).count();
    assert!(red > 0);
}
