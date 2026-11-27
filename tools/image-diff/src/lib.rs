//! The image comparison of the render tests and of the tests that compare
//! renders: how many pixels differ, by pixelmatch's YIQ distance and
//! antialiasing detection.

/// How many pixels of `a` and `b`, `width * height` RGBA pixels each, differ
/// by more than `threshold` (the YIQ distance, from 0 to 1), leaving out the
/// ones that look like antialiasing. The differences are drawn into `diff`,
/// if given, over a faded copy of `a`.
///
/// A semi-transparent pixel compares as its channels blended toward white,
/// `255 + (c - 255) * a`, the blend that the tests' expected images and
/// allowed differences were made with: a transparent pixel matches a white
/// one.
///
/// # Panics
///
/// If a buffer doesn't hold `width * height * 4` bytes.
#[must_use]
pub fn differing_pixels(a: &[u8], b: &[u8], width: u32, height: u32, threshold: f64, diff: Option<&mut [u8]>) -> u64 {
    let (a, b) = (over_white(a), over_white(b));
    let view = |data| blazediff::ImageView { data, width, height };
    let options = blazediff::DiffOptions { threshold, ..Default::default() };
    let result =
        blazediff::diff_rgba(view(&a), view(&b), diff, &options).expect("the buffers hold width * height pixels");
    u64::from(result.count().expect("the images have the same size"))
}

/// `image` made opaque, each channel blended toward white by the alpha.
fn over_white(image: &[u8]) -> Vec<u8> {
    let (pixels, _) = image.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let alpha = f64::from(a) / 255.0;
            let blend = |c: u8| (255.0 + (f64::from(c) - 255.0) * alpha) as u8;
            [blend(r), blend(g), blend(b), 255]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::differing_pixels;

    const SIDE: u32 = 4;

    fn filled(pixel: [u8; 4]) -> Vec<u8> {
        pixel.repeat((SIDE * SIDE) as usize)
    }

    #[test]
    fn equal_images_differ_nowhere() {
        let image: Vec<u8> = (0..SIDE * SIDE * 4).map(|i| (i * 37 % 251) as u8).collect();
        assert_eq!(differing_pixels(&image, &image, SIDE, SIDE, 0.0, None), 0);
    }

    #[test]
    fn a_pixel_of_another_color_differs_and_is_drawn_red() {
        let white = filled([255; 4]);
        let mut other = white.clone();
        let at = (2 * SIDE as usize + 1) * 4;
        other[at..at + 3].fill(0);
        let mut diff = vec![0; white.len()];
        assert_eq!(differing_pixels(&white, &other, SIDE, SIDE, 0.1285, Some(&mut diff)), 1);
        assert_eq!(diff[at..at + 4], [255, 0, 0, 255]);
    }

    #[test]
    fn a_difference_within_the_threshold_is_none() {
        let gray = filled([128, 128, 128, 255]);
        let lighter = filled([132, 132, 132, 255]);
        assert_eq!(differing_pixels(&gray, &lighter, SIDE, SIDE, 0.1285, None), 0);
        assert_eq!(differing_pixels(&gray, &lighter, SIDE, SIDE, 0.0, None), u64::from(SIDE) * u64::from(SIDE));
    }

    #[test]
    fn a_transparent_pixel_matches_a_white_one() {
        let transparent = filled([0; 4]);
        let white = filled([255; 4]);
        assert_eq!(differing_pixels(&transparent, &white, SIDE, SIDE, 0.0, None), 0);
    }
}
