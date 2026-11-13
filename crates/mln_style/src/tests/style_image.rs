use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::image::{Image, ImageContent, ImageStretches};

/// A style image of the given size, without `sdf`.
fn image(
    size: (u32, u32),
    pixel_ratio: f32,
    stretch_x: ImageStretches,
    stretch_y: ImageStretches,
    content: Option<ImageContent>,
) -> Result<Image, String> {
    Image::new(
        "test".to_owned(),
        PremultipliedImage::new(Size::new(size.0, size.1)),
        pixel_ratio,
        false,
        stretch_x,
        stretch_y,
        content,
        None,
        None,
    )
    .map_err(|ex| ex.to_string())
}

fn expect_exception(result: Result<Image, String>, message: &str) {
    match result {
        Ok(_) => panic!("Expected exception"),
        Err(ex) => assert_eq!(message, ex),
    }
}

// gtest: StyleImage.ZeroWidth
#[test]
fn zero_width() {
    expect_exception(image((0, 16), 2.0, vec![], vec![], None), "dimensions may not be zero");
}

// gtest: StyleImage.ZeroHeight
#[test]
fn zero_height() {
    expect_exception(image((16, 0), 2.0, vec![], vec![], None), "dimensions may not be zero");
}

// gtest: StyleImage.ZeroRatio
#[test]
fn zero_ratio() {
    expect_exception(image((16, 16), 0.0, vec![], vec![], None), "pixelRatio may not be <= 0");
}

// gtest: StyleImage.Retina
#[test]
fn retina() {
    let image = image((32, 24), 2.0, vec![], vec![], None).unwrap();
    assert_eq!(32, image.get_image().size.width);
    assert_eq!(24, image.get_image().size.height);
    assert_eq!(2.0, image.get_pixel_ratio());
}

// gtest: StyleImage.FractionalRatio
#[test]
fn fractional_ratio() {
    let image = image((20, 12), 1.5, vec![], vec![], None).unwrap();
    assert_eq!(20, image.get_image().size.width);
    assert_eq!(12, image.get_image().size.height);
    assert_eq!(1.5, image.get_pixel_ratio());
}

// gtest: StyleImage.InvalidStretchX
#[test]
fn invalid_stretch_x() {
    // out of left bound
    expect_exception(image((16, 16), 1.0, vec![(-1.0, 3.0)], vec![], None), "stretchX is out of bounds or overlapping");

    // overlapping
    expect_exception(
        image((16, 16), 1.0, vec![(0.0, 3.0), (2.0, 4.0)], vec![], None),
        "stretchX is out of bounds or overlapping",
    );
}

// gtest: StyleImage.InvalidStretchY
#[test]
fn invalid_stretch_y() {
    // C++ calls `style::Image("test", image, 1, {}, {{...}})`: overload
    // resolution picks the constructor with `bool sdf` (`{}` converts to bool
    // without a user-defined conversion), so `{}` is `sdf` and the stretches
    // are `stretchX`. Hence the "stretchX" message.

    // out of bottom bound
    expect_exception(
        image((16, 16), 1.0, vec![(14.0, 20.0)], vec![], None),
        "stretchX is out of bounds or overlapping",
    );

    // must be sorted
    expect_exception(
        image((16, 16), 1.0, vec![(4.0, 8.0), (2.0, 3.0)], vec![], None),
        "stretchX is out of bounds or overlapping",
    );
}

fn content(left: f32, top: f32, right: f32, bottom: f32) -> ImageContent {
    ImageContent { left, top, right, bottom }
}

// gtest: StyleImage.InvalidContent
#[test]
fn invalid_content() {
    // bottom right out of bounds
    expect_exception(
        image((16, 16), 1.0, vec![], vec![], Some(content(0.0, 0.0, 24.0, 28.0))),
        "content area is invalid",
    );

    // bottom right < top left
    expect_exception(
        image((16, 16), 1.0, vec![], vec![], Some(content(14.0, 14.0, 12.0, 10.0))),
        "content area is invalid",
    );

    // top left out of bounds
    expect_exception(
        image((16, 16), 1.0, vec![], vec![], Some(content(-2.0, -8.0, 12.0, 10.0))),
        "content area is invalid",
    );
}

// gtest: StyleImage.StretchContent
#[test]
fn stretch_content() {
    let image =
        image((16, 16), 1.0, vec![(2.0, 14.0)], vec![(0.0, 4.0), (12.0, 16.0)], Some(content(2.0, 2.0, 14.0, 14.0)))
            .unwrap();
    assert_eq!(16, image.get_image().size.width);
    assert_eq!(16, image.get_image().size.height);
    assert_eq!(1.0, image.get_pixel_ratio());
    assert_eq!(&vec![(2.0_f32, 14.0_f32)], image.get_stretch_x());
    assert_eq!(&vec![(0.0_f32, 4.0_f32), (12.0, 16.0)], image.get_stretch_y());
    assert_eq!(&Some(content(2.0, 2.0, 14.0, 14.0)), image.get_content());
}
