// Ports platform/default/src/mln/util/png_reader.cpp
//
// libpng is replaced by the `png` crate, configured to produce the same
// pixels as the C++ transform chain:
//
//   png_set_expand (palette, gray < 8 bit, tRNS)  -> Transformations::EXPAND
//   png_set_strip_16 (keep the high byte)          -> Transformations::STRIP_16
//   png_set_gray_to_rgb                            -> done below
//   png_set_add_alpha(0xff, PNG_FILLER_AFTER)      -> done below
//
// Neither applies gamma, sRGB or ICC profile conversions (libpng only does when
// asked). Interlaced images are deinterlaced by both.

use mln_util::error::Error;
use mln_util::image::{PremultipliedImage, UnassociatedImage};
use mln_util::premultiply::premultiply;
use mln_util::size::Size;

/// `mln::decodePNG(const uint8_t*, size_t)`.
pub fn decode_png(data: &[u8]) -> Result<PremultipliedImage, Error> {
    if data.len() < 8 {
        return Err(Error::invalid_argument("PNG reader: Could not read image"));
    }
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    if data[..8] != SIGNATURE {
        return Err(Error::invalid_argument("File or stream is not a png"));
    }

    // libpng's messages differ from the png crate's; the prefix is kept.
    let invalid = |e: png::DecodingError| Error::invalid_argument(format!("failed to read invalid png: '{e}'"));

    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(invalid)?;

    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };

    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::invalid_argument("failed to read invalid png: 'image too large'"))?;
    let mut buffer = vec![0u8; buffer_size];
    let output = reader.next_frame(&mut buffer).map_err(invalid)?;
    debug_assert_eq!(output.bit_depth, png::BitDepth::Eight);

    let mut image = UnassociatedImage::new(Size::new(width, height));
    let pixels = width as usize * height as usize;
    let rows = buffer.chunks(output.line_size).take(height as usize);
    let dst_rows = image.data.chunks_exact_mut(width as usize * 4);
    for (src, dst) in rows.zip(dst_rows) {
        match output.color_type {
            // png_set_gray_to_rgb + png_set_add_alpha(0xff)
            png::ColorType::Grayscale => {
                for (s, d) in src.iter().zip(dst.chunks_exact_mut(4)) {
                    d.copy_from_slice(&[*s, *s, *s, 0xff]);
                }
            }
            // png_set_gray_to_rgb
            png::ColorType::GrayscaleAlpha => {
                for (s, d) in src.chunks_exact(2).zip(dst.chunks_exact_mut(4)) {
                    d.copy_from_slice(&[s[0], s[0], s[0], s[1]]);
                }
            }
            // png_set_add_alpha(0xff)
            png::ColorType::Rgb => {
                for (s, d) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                    d.copy_from_slice(&[s[0], s[1], s[2], 0xff]);
                }
            }
            png::ColorType::Rgba => {
                dst.copy_from_slice(&src[..dst.len()]);
            }
            png::ColorType::Indexed => {
                // EXPAND always turns palette images into RGB(A).
                return Err(Error::invalid_argument("failed to read invalid png: 'unexpanded palette'"));
            }
        }
    }
    debug_assert_eq!(image.data.len(), pixels * 4);

    Ok(premultiply(image))
}
