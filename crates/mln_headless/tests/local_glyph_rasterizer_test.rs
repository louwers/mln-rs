// Ports test/text/local_glyph_rasterizer.test.cpp.
//
// LoadLocalCJKGlyph in glyph_manager.test.cpp exercises the
// platform-independent part of LocalGlyphRasterizer. This test renders a
// map with the platform's local glyph rasterizer. Only the NoLocal* cases
// run on every C++ platform, and they are the ones ported: PingFang* are
// Apple-only and NotoSansCJK is Qt-on-Linux-only (platform fonts and
// expectations).

use std::path::PathBuf;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_headless::HeadlessFrontend;
use mln_image::{decode_image, encode_png};
use mln_map::{Map, MapMode, MapOptions};
use mln_map_types::CameraOptions;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{FileSource, Kind, Resource, Response};
use mln_util::geo::EdgeInsets;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

fn read_file(relative: &str) -> Vec<u8> {
    let path = cpp_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `test::getImageDiff` (test/src/mln/test/util.cpp). The C++ writes
/// `<base>/actual.png` next to the fixture; the port writes it to
/// `target/tmp/local_glyph_rasterizer_test` instead.
fn get_image_diff(base: &str, actual: &PremultipliedImage, pixel_threshold: f64) -> f64 {
    let expected = decode_image(&read_file(&format!("{base}/expected.png"))).expect("decode expected.png");

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("local_glyph_rasterizer_test");
    if std::fs::create_dir_all(&dir).is_ok() {
        let name = base.replace('/', "_");
        let _ = std::fs::write(dir.join(format!("{name}-actual.png")), encode_png(actual));
    }

    if expected.size != actual.size {
        return 1.0;
    }

    let pixels = pixelmatch::pixelmatch(
        &actual.data,
        &expected.data,
        expected.size.width as usize,
        expected.size.height as usize,
        None,
        pixel_threshold,
        false,
    );
    pixels as f64 / (f64::from(expected.size.width) * f64::from(expected.size.height))
}

/// `test::checkImage(base, actual, imageThreshold, pixelThreshold)`.
fn check_image(base: &str, actual: &PremultipliedImage, image_threshold: f64, pixel_threshold: f64) {
    let diff = get_image_diff(base, actual, pixel_threshold);
    assert!(diff <= image_threshold, "{base}: difference {diff} > {image_threshold}");
}

/// `LocalGlyphRasterizerTest`.
struct LocalGlyphRasterizerTest {
    _run_loop: RunLoop,
    frontend: HeadlessFrontend,
    map: Map,
}

impl LocalGlyphRasterizerTest {
    /// `LocalGlyphRasterizerTest(fontFamily)`.
    // TODO(port): the C++ passes `fontFamily` to the HeadlessFrontend's
    // LocalGlyphRasterizer; the port's platform/default rasterizer refuses
    // every glyph (as C++'s does without a font family), so only `None` is
    // supported here.
    fn new(font_family: Option<&str>) -> Self {
        assert!(font_family.is_none(), "local font families are not ported");
        let run_loop = RunLoop::new();
        let file_source = StubFileSource::new(ResponseType::Asynchronous);
        file_source.set_response(Kind::Glyphs, |resource: &Resource| {
            assert_eq!(Kind::Glyphs, resource.kind);
            let mut response = Response::new();
            response.data = Some(Arc::new(read_file("test/fixtures/resources/glyphs.pbf")));
            Some(response)
        });
        let file_source: Arc<dyn FileSource> = Arc::new(file_source);

        // `HeadlessFrontend(1, NoFlush, ContextMode::Unique, fontFamily)`:
        // the default size is 256x256.
        let frontend = HeadlessFrontend::new(Size::new(256, 256), 1.0).expect("a headless frontend");
        // `MapAdapter map{frontend, nullObserver, fileSource, MapOptions()...}`.
        let map = Map::with_file_source(
            file_source,
            MapOptions { map_mode: MapMode::Static, size: frontend.get_size(), ..Default::default() },
        )
        .unwrap();
        frontend.attach(&map);
        LocalGlyphRasterizerTest { _run_loop: run_loop, frontend, map }
    }

    fn check_rendering(&mut self, name: &str, image_match_pixels_threshold: f64, pixel_match_threshold: f64) {
        let image = self.frontend.render(&mut self.map).expect("render").image;
        check_image(
            &format!("test/fixtures/local_glyphs/{name}"),
            &image,
            image_match_pixels_threshold,
            pixel_match_threshold,
        );
    }
}

fn mixed_json() -> String {
    String::from_utf8(read_file("test/fixtures/local_glyphs/mixed.json")).unwrap()
}

// gtest: LocalGlyphRasterizer.NoLocal
#[test]
fn no_local() {
    // Expectation: without any local fonts set, and without any CJK glyphs
    // provided, the output should just contain basic latin characters.
    let mut test = LocalGlyphRasterizerTest::new(None);
    test.map.style_mut().load_json(&mixed_json());
    test.check_rendering("no_local", 0.001, 0.1);
}

// gtest: LocalGlyphRasterizer.NoLocalWithContentInsets
#[test]
fn no_local_with_content_insets() {
    // Expectations: content insets imply rendering to offsetted viewport
    // center. Rendered text should be on the same offset and keep the same
    // size as with no offset.
    let mut test = LocalGlyphRasterizerTest::new(None);
    let view_size = test.frontend.get_size();
    test.map.style_mut().load_json(&mixed_json());

    // Expected image was created using center offset as in the line below -
    // content insets, with no pitch defined, should produce the same output.
    // test.map.moveBy({ viewSize.width * 0.25, 0 });

    test.map.jump_to(&CameraOptions {
        padding: Some(EdgeInsets { top: 0.0, left: f64::from(view_size.width) * 0.25 * 2.0, bottom: 0.0, right: 0.0 }),
        ..Default::default()
    });
    test.check_rendering("no_local_with_content_insets", 0.001, 0.1);
}

// gtest: LocalGlyphRasterizer.NoLocalWithContentInsetsAndPitch
#[test]
fn no_local_with_content_insets_and_pitch() {
    // Expectations: content insets imply rendering to offsetted viewport
    // center. Rendered text should be on the same offset and keep the same
    // size as with no offset.
    let mut test = LocalGlyphRasterizerTest::new(None);
    let view_size = test.frontend.get_size();
    test.map.style_mut().load_json(&mixed_json());

    // Expected image was verified using no-padding render, offsetted to right
    // using bitmap editor.

    test.map.jump_to(&CameraOptions {
        pitch: Some(45.0),
        padding: Some(EdgeInsets { top: 0.0, left: f64::from(view_size.width) * 0.4, bottom: 0.0, right: 0.0 }),
        ..Default::default()
    });
    test.check_rendering("no_local_with_content_insets_and_pitch", 0.001, 0.1);
}
