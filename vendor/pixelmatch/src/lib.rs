// Ports vendor/maplibre-native-base/deps/pixelmatch-cpp/include/mapbox/pixelmatch.hpp
//
//! The pixelmatch image diff used by the C++ render-test runner
//! (`render-test/runner.cpp`, `mapbox::pixelmatch(..., 0.1285)`).
//!
//! A line-by-line port: the same `double` math in the same order, the same
//! `double` → `uint8_t` truncations (`as u8` truncates toward zero; every
//! converted value is in `[0, 256)`, where C++ conversion is defined), and the
//! same `size_t` neighbourhood logic. Images are RGBA, 4 bytes per pixel,
//! row-major, `width * height * 4` bytes.

pub mod detail {
    /// Blend semi-transparent color with white.
    ///
    /// C++: `uint8_t blend(uint8_t c, double a) { return 255 + (c - 255) * a; }`
    /// (`c - 255` is an `int`, promoted to `double`; the `double` result is
    /// truncated to `uint8_t`).
    #[inline]
    #[must_use]
    pub fn blend(c: u8, a: f64) -> u8 {
        (255.0 + f64::from(i32::from(c) - 255) * a) as u8
    }

    /// C++ passes a `double` (from [`gray_pixel`]) to `blend(uint8_t, double)`,
    /// which truncates it to `uint8_t` first.
    #[inline]
    pub(crate) fn blend_f64(c: f64, a: f64) -> u8 {
        blend(c as u8, a)
    }

    #[inline]
    #[must_use]
    pub fn rgb2y(r: u8, g: u8, b: u8) -> f64 {
        f64::from(r) * 0.298_895_31 + f64::from(g) * 0.586_622_47 + f64::from(b) * 0.114_482_23
    }

    #[inline]
    #[must_use]
    pub fn rgb2i(r: u8, g: u8, b: u8) -> f64 {
        f64::from(r) * 0.595_977_99 - f64::from(g) * 0.274_176_10 - f64::from(b) * 0.321_801_89
    }

    #[inline]
    #[must_use]
    pub fn rgb2q(r: u8, g: u8, b: u8) -> f64 {
        f64::from(r) * 0.211_470_17 - f64::from(g) * 0.522_617_11 + f64::from(b) * 0.311_146_94
    }

    /// Calculate color difference according to the paper "Measuring perceived
    /// color difference using YIQ NTSC transmission color space in mobile
    /// applications" by Y. Kotsarenko and F. Ramos.
    #[must_use]
    pub fn color_delta(img1: &[u8], img2: &[u8], k: usize, m: usize, y_only: bool) -> f64 {
        let a1 = f64::from(img1[k + 3]) / 255.0;
        let a2 = f64::from(img2[m + 3]) / 255.0;

        let r1 = blend(img1[k], a1);
        let g1 = blend(img1[k + 1], a1);
        let b1 = blend(img1[k + 2], a1);

        let r2 = blend(img2[m], a2);
        let g2 = blend(img2[m + 1], a2);
        let b2 = blend(img2[m + 2], a2);

        let y = rgb2y(r1, g1, b1) - rgb2y(r2, g2, b2);

        if y_only {
            return y; // brightness difference only
        }

        let i = rgb2i(r1, g1, b1) - rgb2i(r2, g2, b2);
        let q = rgb2q(r1, g1, b1) - rgb2q(r2, g2, b2);

        0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q
    }

    #[inline]
    pub fn draw_pixel(output: &mut [u8], pos: usize, r: u8, g: u8, b: u8) {
        output[pos] = r;
        output[pos + 1] = g;
        output[pos + 2] = b;
        output[pos + 3] = 255;
    }

    #[must_use]
    pub fn gray_pixel(img: &[u8], i: usize) -> f64 {
        let a = f64::from(img[i + 3]) / 255.0;
        let r = blend(img[i], a);
        let g = blend(img[i + 1], a);
        let b = blend(img[i + 2], a);
        rgb2y(r, g, b)
    }

    /// Check if a pixel is likely a part of anti-aliasing; based on
    /// "Anti-aliased Pixel and Intensity Slope Detector" paper by
    /// V. Vysniauskas, 2009.
    ///
    /// `img2` is `nullptr` in the C++ recursive calls.
    #[must_use]
    pub fn antialiased(img: &[u8], x1: usize, y1: usize, width: usize, height: usize, img2: Option<&[u8]>) -> bool {
        let x0 = if x1 > 0 { x1 - 1 } else { 0 };
        let y0 = if y1 > 0 { y1 - 1 } else { 0 };
        // `std::min(x1 + 1, width - 1)` on `size_t`.
        let x2 = std::cmp::min(x1.wrapping_add(1), width.wrapping_sub(1));
        let y2 = std::cmp::min(y1.wrapping_add(1), height.wrapping_sub(1));
        let pos = (y1 * width + x1) * 4;
        let mut zeroes: u64 = 0;
        let mut positives: u64 = 0;
        let mut negatives: u64 = 0;
        let mut min: f64 = 0.0;
        let mut max: f64 = 0.0;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (0usize, 0usize, 0usize, 0usize);

        // go through 8 adjacent pixels
        for x in x0..=x2 {
            for y in y0..=y2 {
                if x == x1 && y == y1 {
                    continue;
                }

                // brightness delta between the center pixel and adjacent one
                let delta = color_delta(img, img, pos, (y * width + x) * 4, true);

                // count the number of equal, darker and brighter adjacent pixels
                if delta == 0.0 {
                    zeroes += 1;
                } else if delta < 0.0 {
                    negatives += 1;
                } else if delta > 0.0 {
                    positives += 1;
                }

                // if found more than 2 equal siblings, it's definitely not anti-aliasing
                if zeroes > 2 {
                    return false;
                }

                if img2.is_none() {
                    continue;
                }

                // remember the darkest pixel
                if delta < min {
                    min = delta;
                    min_x = x;
                    min_y = y;
                }
                // remember the brightest pixel
                if delta > max {
                    max = delta;
                    max_x = x;
                    max_y = y;
                }
            }
        }

        let Some(img2) = img2 else {
            return true;
        };

        // if there are no both darker and brighter pixels among siblings, it's not anti-aliasing
        if negatives == 0 || positives == 0 {
            return false;
        }

        // if either the darkest or the brightest pixel has more than 2 equal siblings in both images
        // (definitely not anti-aliased), this pixel is anti-aliased
        (!antialiased(img, min_x, min_y, width, height, None) && !antialiased(img2, min_x, min_y, width, height, None))
            || (!antialiased(img, max_x, max_y, width, height, None)
                && !antialiased(img2, max_x, max_y, width, height, None))
    }
}

/// `mapbox::pixelmatch(img1, img2, width, height, output, threshold, includeAA)`.
///
/// C++ defaults: `output = nullptr`, `threshold = 0.1`, `includeAA = false`.
/// Returns the number of mismatched pixels. `img1`, `img2` (and `output`, if
/// given) must hold at least `width * height * 4` bytes; shorter slices
/// panic, where C++ reads out of bounds.
#[must_use]
pub fn pixelmatch(
    img1: &[u8],
    img2: &[u8],
    width: usize,
    height: usize,
    mut output: Option<&mut [u8]>,
    threshold: f64,
    include_aa: bool,
) -> u64 {
    use detail::{antialiased, blend_f64, color_delta, draw_pixel, gray_pixel};

    // maximum acceptable square distance between two colors;
    // 35215 is the maximum possible value for the YIQ difference metric
    let max_delta = 35215.0 * threshold * threshold;
    let mut diff: u64 = 0;

    // compare each pixel of one image against the other one
    for y in 0..height {
        for x in 0..width {
            let pos = (y * width + x) * 4;

            // squared YUV distance between colors at this pixel position
            let delta = color_delta(img1, img2, pos, pos, false);

            // the color difference is above the threshold
            if delta > max_delta {
                // check it's a real rendering difference or just anti-aliasing
                if !include_aa
                    && (antialiased(img1, x, y, width, height, Some(img2))
                        || antialiased(img2, x, y, width, height, Some(img1)))
                {
                    // one of the pixels is anti-aliasing; draw as yellow and do not count as difference
                    if let Some(out) = output.as_deref_mut() {
                        draw_pixel(out, pos, 255, 255, 0);
                    }
                } else {
                    // found substantial difference not caused by anti-aliasing; draw it as red
                    if let Some(out) = output.as_deref_mut() {
                        draw_pixel(out, pos, 255, 0, 0);
                    }
                    diff += 1;
                }
            } else if let Some(out) = output.as_deref_mut() {
                // pixels are similar; draw background as grayscale image blended with white
                let val = blend_f64(gray_pixel(img1, pos), 0.1);
                draw_pixel(out, pos, val, val, val);
            }
        }
    }

    // return the number of different pixels
    diff
}

#[cfg(test)]
mod tests {
    use super::detail::*;
    use super::*;

    #[test]
    fn blend_truncates_like_cpp() {
        // 255 + (0 - 255) * 0.5 = 127.5 -> 127
        assert_eq!(blend(0, 0.5), 127);
        assert_eq!(blend(255, 0.0), 255);
        assert_eq!(blend(0, 1.0), 0);
        assert_eq!(blend(10, 1.0), 10);
        // gray_pixel of white is 255.0000025..., truncated to 255 by the
        // implicit double -> uint8_t conversion.
        let white = [255u8, 255, 255, 255];
        assert!(gray_pixel(&white, 0) > 255.0);
        assert_eq!(blend_f64(gray_pixel(&white, 0), 0.1), 255);
    }

    #[test]
    fn identical_images_match() {
        let w = 7;
        let h = 5;
        let img: Vec<u8> = (0..w * h * 4).map(|i| (i * 37 % 251) as u8).collect();
        let mut out = vec![0u8; w * h * 4];
        assert_eq!(pixelmatch(&img, &img, w, h, Some(&mut out), 0.1285, false), 0);
        // Every output pixel is gray and opaque.
        for px in out.chunks(4) {
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn single_pixel_difference() {
        let w = 4;
        let h = 4;
        let a = vec![255u8; w * h * 4];
        let mut b = a.clone();
        // Make pixel (1, 2) opaque black.
        let pos = (2 * w + 1) * 4;
        b[pos..pos + 3].copy_from_slice(&[0, 0, 0]);
        let mut out = vec![0u8; w * h * 4];
        assert_eq!(pixelmatch(&a, &b, w, h, Some(&mut out), 0.1, false), 1);
        assert_eq!(&out[pos..pos + 4], &[255, 0, 0, 255]);
        assert_eq!(pixelmatch(&a, &b, w, h, None, 0.1, true), 1);
    }
}
