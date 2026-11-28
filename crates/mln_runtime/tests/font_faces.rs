//! Text that a map shapes with its fallback font faces, in a style without
//! glyph PBFs.

use std::sync::Arc;

use mln_engine::storage::Response;
use mln_runtime::MapOptions;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions, not_found, serve};
use crate::support::wait;

/// A font of Khmer, with the glyphs of [`KHMER_TEXT`].
const KHMER_FONT: &[u8] = include_bytes!("../../mln_engine/tests/fixtures/khmer.ttf");

/// "ខ្មែរ": a subscript consonant and a split vowel, which shaping rearranges
/// and positions.
const KHMER_TEXT: &str = r#""ខ្មែរ""#;

/// A still of a style without glyph PBFs whose label's text is
/// `text_field`, with a red image "icon", on a map whose one fallback font
/// face is [`KHMER_FONT`], for Khmer, and which draws nothing from the
/// system's fonts.
fn render(text_field: &str) -> PremultipliedImage {
    let map = StillMap::new(StillOptions {
        map: MapOptions {
            fallback_font_faces: Some(r#"[{"url": "fonts/khmer.ttf", "unicode-range": ["U+1780-17FF"]}]"#.to_owned()),
            system_fonts: false,
            local_ideographs: false,
            ..MapOptions::default()
        },
        ..StillOptions::default()
    });
    let requests = serve(&map.runtime, |resource| match resource.url.as_str() {
        "fonts/khmer.ttf" => Response { data: Some(Arc::new(KHMER_FONT.to_vec())), ..Response::new() },
        _ => not_found(),
    });
    map.load_json(&format!(
        r#"{{
        "version": 8,
        "sources": {{"point": {{"type": "geojson", "data": {{
            "type": "Feature", "properties": {{}}, "geometry": {{"type": "Point", "coordinates": [0, 0]}}}}}}}},
        "layers": [{{"id": "symbol", "type": "symbol", "source": "point",
                    "layout": {{"text-field": {text_field}, "text-font": ["Any Font"], "text-size": 48}}}}]
    }}"#
    ));
    let icon = PremultipliedImage::from_vec(Size::new(16, 16), [255, 0, 0, 255].repeat(16 * 16));
    let icon =
        mln_style::image::Impl::new("icon".to_owned(), icon, 1.0, false, Vec::new(), Vec::new(), None, None, None);
    wait(map.map.set_style_image(icon.expect("a style image")));
    let image = map.render();
    assert!(requests.all().iter().any(|(_, url)| url == "fonts/khmer.ttf"), "the map loads its fallback font face");
    image
}

/// An image in a formatted label beside text that the fallback font face
/// shapes draws as the image, not as text.
#[test]
fn an_image_beside_shaped_text_stays_an_image() {
    let field = format!(r#"["format", {KHMER_TEXT}, {{}}, ["image", "icon"], {{}}]"#);
    let image = render(&field);
    let pixels = image.data.as_chunks::<4>().0;
    let red = pixels.iter().filter(|pixel| pixel[0] > 200 && pixel[1] < 50 && pixel[3] > 200).count();
    assert!(red > 0, "the image draws");
    let text = pixels.iter().filter(|pixel| pixel[0] < 50 && pixel[3] > 200).count();
    assert!(text > 0, "the text draws");
}
