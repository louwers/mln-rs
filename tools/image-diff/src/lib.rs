//! The image comparison of the render tests and of the tests that compare
//! renders: how many pixels differ, by pixelmatch's YIQ distance and
//! antialiasing detection.

/// How many pixels of `a` and `b`, `width * height` RGBA pixels each, differ
/// by more than `threshold` (the YIQ distance, from 0 to 1), leaving out the
/// ones that look like antialiasing. The differences are drawn into `diff`,
/// if given, over a faded copy of `a`.
///
/// The buffers hold premultiplied pixels, which compare as composited over
/// white: a transparent pixel matches a white one.
///
/// # Panics
///
/// If a buffer doesn't hold `width * height * 4` bytes.
#[must_use]
pub fn differing_pixels(a: &[u8], b: &[u8], width: u32, height: u32, threshold: f64, diff: Option<&mut [u8]>) -> u64 {
    let image = |data| blazediff::Image { data: over_white(data), width, height };
    let mut output = diff.is_some().then(|| blazediff::Image::new(width, height));
    let options = blazediff::DiffOptions { threshold, ..Default::default() };
    let result = blazediff::diff(&image(a), &image(b), output.as_mut(), &options)
        .expect("the buffers hold width * height pixels");
    if let (Some(diff), Some(output)) = (diff, output) {
        diff.copy_from_slice(&output.data);
    }
    u64::from(result.count().expect("the images have the same size"))
}

/// The premultiplied `image` composited over opaque white:
/// `c + 255 * (1 - a)` per channel, at most 255 (for a color brighter than
/// its alpha).
fn over_white(image: &[u8]) -> Vec<u8> {
    let (pixels, _) = image.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let blend = |c: u8| c.saturating_add(255 - a);
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
    fn a_semi_transparent_pixel_compares_as_composited_over_white() {
        // Half-transparent red, premultiplied, over white is (255, 127, 127).
        let red = filled([128, 0, 0, 128]);
        assert_eq!(differing_pixels(&red, &filled([255, 127, 127, 255]), SIDE, SIDE, 0.0, None), 0);
        assert_eq!(
            differing_pixels(&red, &filled([255, 128, 128, 255]), SIDE, SIDE, 0.0, None),
            u64::from(SIDE * SIDE)
        );
        // Half-transparent white, premultiplied, over white is white.
        assert_eq!(differing_pixels(&filled([128; 4]), &filled([255; 4]), SIDE, SIDE, 0.0, None), 0);
    }

    #[test]
    fn a_transparent_pixel_matches_a_white_one() {
        let transparent = filled([0; 4]);
        let white = filled([255; 4]);
        assert_eq!(differing_pixels(&transparent, &white, SIDE, SIDE, 0.0, None), 0);
    }
}
