// libwebp (`WebPGetInfo` + `WebPDecodeRGBAInto`) is replaced by the
// `image-webp` crate: lossless (VP8L) images
// decode to the same pixels; lossy (VP8) images use its bilinear ("fancy")
// chroma upsampling, which libwebp also uses by default.

use std::io::Cursor;

use mln_style::bitmap::{PremultipliedImage, UnassociatedImage};
use mln_style::size::Size;
use mln_util::premultiply::premultiply;

use crate::log_target;

/// Decodes WebP `data`. Logs a warning and returns an empty image when the
/// data can't be decoded, as C++ does.
#[must_use]
pub(crate) fn decode_webp(data: &[u8]) -> PremultipliedImage {
    let Ok(mut decoder) = image_webp::WebPDecoder::new(Cursor::new(data)) else {
        log::warn!(target: log_target::IMAGE, "Failed to decode WebP image header!");
        return PremultipliedImage::default();
    };

    let (width, height) = decoder.dimensions();
    let Some(buffer_size) = decoder.output_buffer_size() else {
        log::warn!(target: log_target::IMAGE, "Failed to decode WebP image contents!");
        return PremultipliedImage::default();
    };
    let mut buffer = vec![0u8; buffer_size];
    if decoder.read_image(&mut buffer).is_err() {
        log::warn!(target: log_target::IMAGE, "Failed to decode WebP image contents!");
        return PremultipliedImage::default();
    }

    // `WebPDecodeRGBAInto`: RGBA, opaque alpha for images without alpha.
    let rgba = if decoder.has_alpha() {
        buffer
    } else {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for rgb in buffer.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        rgba
    };

    premultiply(UnassociatedImage::from_vec(Size::new(width, height), rgba))
}
