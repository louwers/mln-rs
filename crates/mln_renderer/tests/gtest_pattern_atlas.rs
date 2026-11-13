use std::path::PathBuf;

use mln_image::decode_image;
use mln_renderer::pattern_atlas::PatternAtlas;
use mln_style_types::image::Impl as ImageImpl;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

/// The path of `test/fixtures/...` in the copy of the C++ files (testdata/).
fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative)
}

fn read_file(relative: &str) -> Vec<u8> {
    let path = cpp_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// test/src/mln/test/util.cpp `test::checkImage(base, actual)` with the
/// default thresholds (image 0, pixel 0): `getImageDiff` against
/// `<base>/expected.png`. The fixtures are read in place, so the
/// actual.png/diff.png outputs (`!TEST_READ_ONLY`) are not written.
fn check_image(base: &str, actual: &PremultipliedImage) {
    let image_threshold = 0.0;
    let pixel_threshold = 0.0;
    let expected = decode_image(&read_file(&format!("{base}/expected.png"))).expect("expected.png");
    let diff = if expected.size == actual.size {
        let pixels = pixelmatch::pixelmatch(
            &actual.data,
            &expected.data,
            expected.size.width as usize,
            expected.size.height as usize,
            None,
            pixel_threshold,
            false,
        );
        pixels as f64 / f64::from(expected.size.width * expected.size.height)
    } else {
        1.0
    };
    assert!(diff <= image_threshold, "{base}: image diff {diff} > {image_threshold}");
}

fn make_image(id: &str, image: PremultipliedImage, pixel_ratio: f32, sdf: bool) -> ImageImpl {
    ImageImpl::new(id.to_owned(), image, pixel_ratio, sdf, Vec::new(), Vec::new(), None, None, None).expect("image")
}

// gtest: PatternAtlas.Basic
#[test]
fn basic() {
    let mut pattern_atlas = PatternAtlas::new();

    let images = mln_sprite::parse_sprite(
        "default",
        &read_file("test/fixtures/annotations/emerald.png"),
        &read_file("test/fixtures/annotations/emerald.json"),
    )
    .expect("parseSprite");
    for image in &images {
        if image.id == "metro" {
            assert!(pattern_atlas.add_pattern(image).is_some());
        }
    }
    let found = pattern_atlas.get_pattern("metro");
    assert!(found.is_some());

    let metro = found.unwrap();
    assert_eq!(1, metro.tl()[0]);
    assert_eq!(1, metro.tl()[1]);
    assert_eq!(19, metro.br()[0]);
    assert_eq!(19, metro.br()[1]);
    assert_eq!(18.0, metro.display_size()[0]);
    assert_eq!(18.0, metro.display_size()[1]);
    assert_eq!(1.0f32, metro.pixel_ratio);
    assert!(!metro.sdf);
    assert_eq!(pattern_atlas.get_pixel_size(), pattern_atlas.get_atlas_image_for_test().size);

    check_image("test/fixtures/image_manager/basic", pattern_atlas.get_atlas_image_for_test());
}

// gtest: PatternAtlas.PreservesSDFMetadata
#[test]
fn preserves_sdf_metadata() {
    let mut pattern_atlas = PatternAtlas::new();

    let sdf_image = make_image("sdf", PremultipliedImage::new(Size::new(8, 8)), 1.0, true);
    let rgba_image = make_image("rgba", PremultipliedImage::new(Size::new(8, 8)), 1.0, false);

    let sdf_position = pattern_atlas.add_pattern(&sdf_image);
    let rgba_position = pattern_atlas.add_pattern(&rgba_image);

    assert!(sdf_position.is_some());
    assert!(rgba_position.is_some());
    assert!(sdf_position.unwrap().sdf);
    assert!(!rgba_position.unwrap().sdf);
}

// gtest: PatternAtlas.Updates
#[test]
fn updates() {
    let mut pattern_atlas = PatternAtlas::new();

    let mut image_a = PremultipliedImage::new(Size::new(16, 12));
    image_a.fill(255);

    let added = pattern_atlas.add_pattern(&make_image("one", image_a, 1.0, false));
    assert!(added.is_some());
    let found = pattern_atlas.get_pattern("one");
    assert!(found.is_some());
    assert_eq!(added.unwrap().padded_rect, found.as_ref().unwrap().padded_rect);

    let a = found.unwrap();
    assert_eq!(1, a.tl()[0]);
    assert_eq!(1, a.tl()[1]);
    assert_eq!(17, a.br()[0]);
    assert_eq!(13, a.br()[1]);
    assert_eq!(16.0, a.display_size()[0]);
    assert_eq!(12.0, a.display_size()[1]);
    assert_eq!(1.0f32, a.pixel_ratio);
    check_image("test/fixtures/image_manager/updates_before", pattern_atlas.get_atlas_image_for_test());

    let image_b = make_image("one", PremultipliedImage::new(Size::new(5, 5)), 1.0, false);
    assert!(pattern_atlas.add_pattern(&image_b).is_none()); // Already added.

    pattern_atlas.remove_pattern("one");
    assert!(pattern_atlas.get_pattern("one").is_none());
    assert!(pattern_atlas.add_pattern(&image_b).is_some());

    let b = pattern_atlas.get_pattern("one").expect("one");
    assert_eq!(1, b.tl()[0]);
    assert_eq!(1, b.tl()[1]);
    assert_eq!(6, b.br()[0]);
    assert_eq!(6, b.br()[1]);
    assert_eq!(5.0, b.display_size()[0]);
    assert_eq!(5.0, b.display_size()[1]);
    assert_eq!(1.0f32, b.pixel_ratio);
    check_image("test/fixtures/image_manager/updates_after", pattern_atlas.get_atlas_image_for_test());
}
