//! A camera's padding, its content insets, moves the center that the map
//! draws around, and keeps the size of what it draws there. The labels come
//! from glyph PBFs alone: the map draws no glyph from local fonts.

use mln_engine::camera::CameraOptions;
use mln_engine::geo::EdgeInsets;
use mln_engine::storage::Kind;

use crate::support::still::{StillMap, check_image, file_response, not_found, read_file, serve};

const SIDE: f64 = 256.0;

/// A still map of `local_glyphs/mixed.json`, whose glyphs come from the
/// fixtures' glyph PBF, seen with `camera` jumped to after the style's own.
fn render_mixed(camera: CameraOptions) -> mln_style::bitmap::PremultipliedImage {
    let map = StillMap::still();
    serve(&map.runtime, |resource| match resource.kind {
        Kind::Glyphs => file_response("test/fixtures/resources/glyphs.pbf"),
        _ => not_found(),
    });
    map.load_json(&read_file("test/fixtures/local_glyphs/mixed.json"));
    map.jump(camera);
    map.render()
}

// gtest: LocalGlyphRasterizer.NoLocalWithContentInsets
#[test]
fn no_local_with_content_insets() {
    // Expectations: content insets imply rendering to offsetted viewport
    // center. Rendered text should be on the same offset and keep the same
    // size as with no offset.
    //
    // The expected image is of the center moved by a quarter of the view's
    // width (C++'s `moveBy({ viewSize.width * 0.25, 0 })`): content insets,
    // with no pitch, draw the same.
    let image = render_mixed(CameraOptions {
        padding: Some(EdgeInsets { top: 0.0, left: SIDE * 0.25 * 2.0, bottom: 0.0, right: 0.0 }),
        ..CameraOptions::default()
    });
    check_image("test/fixtures/local_glyphs/no_local_with_content_insets", &image, 0.001, 0.1);
}

// gtest: LocalGlyphRasterizer.NoLocalWithContentInsetsAndPitch
#[test]
fn no_local_with_content_insets_and_pitch() {
    // Expectations: content insets imply rendering to offsetted viewport
    // center. Rendered text should be on the same offset and keep the same
    // size as with no offset.
    //
    // The expected image was verified against one drawn without padding,
    // moved to the right in a bitmap editor.
    let image = render_mixed(CameraOptions {
        pitch: Some(45.0),
        padding: Some(EdgeInsets { top: 0.0, left: SIDE * 0.4, bottom: 0.0, right: 0.0 }),
        ..CameraOptions::default()
    });
    check_image("test/fixtures/local_glyphs/no_local_with_content_insets_and_pitch", &image, 0.001, 0.1);
}
