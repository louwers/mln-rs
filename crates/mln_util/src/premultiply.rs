// Ports src/mln/util/premultiply.cpp and include/mln/util/premultiply.hpp

use std::num::NonZeroU32;

use crate::image::{PremultipliedImage, UnassociatedImage};

/// `util::premultiply(UnassociatedImage&&)`: `c = (c * a + 127) / 255` in
/// `int` arithmetic.
#[must_use]
pub fn premultiply(src: UnassociatedImage) -> PremultipliedImage {
    let (size, mut data) = src.into_parts();
    let bytes = PremultipliedImage::CHANNELS * size.width as usize * size.height as usize;
    for px in data.as_chunks_mut::<4>().0.iter_mut().take(bytes / 4) {
        let a = u32::from(px[3]);
        px[0] = ((u32::from(px[0]) * a + 127) / 255) as u8;
        px[1] = ((u32::from(px[1]) * a + 127) / 255) as u8;
        px[2] = ((u32::from(px[2]) * a + 127) / 255) as u8;
    }
    PremultipliedImage::from_vec(size, data)
}

/// `util::unpremultiply(PremultipliedImage&&)`: for `a != 0`,
/// `c = static_cast<uint8_t>((255 * c + (a / 2)) / a)`. The cast wraps
/// modulo 256 when `c > a` (not a valid premultiplied pixel); `as u8` does
/// the same.
#[must_use]
pub fn unpremultiply(src: PremultipliedImage) -> UnassociatedImage {
    let (size, mut data) = src.into_parts();
    let bytes = UnassociatedImage::CHANNELS * size.width as usize * size.height as usize;
    for px in data.as_chunks_mut::<4>().0.iter_mut().take(bytes / 4) {
        if let Some(a) = NonZeroU32::new(u32::from(px[3])) {
            let half = a.get() / 2;
            px[0] = ((255 * u32::from(px[0]) + half) / a) as u8;
            px[1] = ((255 * u32::from(px[1]) + half) / a) as u8;
            px[2] = ((255 * u32::from(px[2]) + half) / a) as u8;
        }
    }
    UnassociatedImage::from_vec(size, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size::Size;

    // Ports TEST(Image, Premultiply) from test/util/image.test.cpp.
    // gtest: Image.Premultiply
    #[test]
    fn premultiply_() {
        let mut rgba = UnassociatedImage::new(Size::new(1, 1));
        rgba.data.copy_from_slice(&[255, 254, 253, 128]);

        let image = premultiply(std::mem::take(&mut rgba));
        assert_eq!(image.data[0], 128);
        assert_eq!(image.data[1], 127);
        assert_eq!(image.data[2], 127);
        assert_eq!(image.data[3], 128);
        assert_eq!(image.size.width, 1);
        assert_eq!(image.size.height, 1);
        assert_eq!(rgba.size.width, 0);
        assert_eq!(rgba.size.height, 0);
    }

    // Not in the C++ suite.
    #[test]
    fn unpremultiply_() {
        let image =
            PremultipliedImage::from_vec(Size::new(3, 1), vec![128, 127, 127, 128, 10, 20, 30, 0, 200, 0, 0, 100]);
        let rgba = unpremultiply(image);
        // (255 * 128 + 64) / 128 = 255, (255 * 127 + 64) / 128 = 253
        assert_eq!(&rgba.data[0..4], &[255, 253, 253, 128]);
        // a == 0 leaves the color untouched.
        assert_eq!(&rgba.data[4..8], &[10, 20, 30, 0]);
        // (255 * 200 + 50) / 100 = 510, wrapped to 254 by static_cast<uint8_t>.
        assert_eq!(&rgba.data[8..12], &[254, 0, 0, 100]);
    }

    // Not in the C++ suite: premultiply/unpremultiply round trip over all
    // (value, alpha) pairs matches the closed-form C++ integer math.
    #[test]
    fn exhaustive_against_formula() {
        let mut data = Vec::with_capacity(256 * 256 * 4);
        for a in 0..=255u8 {
            for c in 0..=255u8 {
                data.extend_from_slice(&[c, c, c, a]);
            }
        }
        let pre = premultiply(UnassociatedImage::from_vec(Size::new(256, 256), data));
        for (i, px) in pre.data.chunks(4).enumerate() {
            let (a, c) = ((i / 256) as i32, (i % 256) as i32);
            assert_eq!(i32::from(px[0]), (c * a + 127) / 255);
        }
        let un = unpremultiply(pre.clone());
        for (px, pre_px) in un.data.chunks(4).zip(pre.data.chunks(4)) {
            let (a, c) = (i32::from(pre_px[3]), i32::from(pre_px[0]));
            let expected = if a != 0 { ((255 * c + a / 2) / a) as u8 } else { c as u8 };
            assert_eq!(px[0], expected);
        }
    }
}
