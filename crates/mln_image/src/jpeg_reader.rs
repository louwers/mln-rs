// libjpeg is replaced by zune-jpeg, asked for libjpeg's default output color
// space, of which C++ keeps the first three components (the one, replicated,
// of a grayscale image):
//
//   grayscale    -> grayscale
//   YCbCr, RGB   -> RGB
//   CMYK         -> CMYK, as stored
//   YCCK         -> CMYK, converted below as libjpeg's ycck_cmyk_convert does
//
// C++ throws on libjpeg's first warning, not only on its errors, so the
// decoder runs in strict mode, which fails where libjpeg warns (truncated
// data, stray bytes or markers).

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use zune_jpeg::JpegDecoder;
use zune_jpeg::errors::DecodeErrors;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

pub(crate) fn decode_jpeg(data: &[u8]) -> Result<PremultipliedImage, Error> {
    let invalid = |e: DecodeErrors| Error::invalid_argument(format!("JPEG Reader: libjpeg could not read image: {e}"));
    let options = DecoderOptions::default().set_strict_mode(true);

    // The output color space must be set before the headers are read, and
    // depends on the input color space they declare.
    let mut headers = JpegDecoder::new_with_options(ZCursor::new(data), options);
    headers.decode_headers().map_err(invalid)?;
    let output = match headers.input_colorspace() {
        Some(ColorSpace::Luma) => ColorSpace::Luma,
        Some(ColorSpace::YCbCr | ColorSpace::RGB) => ColorSpace::RGB,
        Some(ColorSpace::CMYK) => ColorSpace::CMYK,
        Some(ColorSpace::YCCK) => ColorSpace::YCCK,
        _ => return Err(Error::invalid_argument("JPEG Reader: failed to read unknown color space")),
    };

    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options.jpeg_set_out_colorspace(output));
    let pixels = decoder.decode().map_err(invalid)?;
    let (width, height) = decoder.dimensions().unwrap_or_default();
    if width == 0 || height == 0 {
        return Err(Error::invalid_argument("JPEG Reader: failed to read image size"));
    }

    // zune-jpeg caps both dimensions far below u32::MAX.
    let mut image = PremultipliedImage::new(Size::new(width as u32, height as u32));
    let components = output.num_components();
    for (src, dst) in pixels.chunks_exact(components).zip(image.data.as_chunks_mut::<4>().0) {
        let [r, g, b] = match output {
            ColorSpace::Luma => [src[0]; 3],
            ColorSpace::YCCK => ycck_to_cmy(src[0], src[1], src[2]),
            _ => [src[0], src[1], src[2]],
        };
        *dst = [r, g, b, 0xff];
    }
    Ok(image)
}

/// The C, M and Y that libjpeg's `ycck_cmyk_convert` makes of a YCCK pixel:
/// the complement of the RGB of its YCC, with libjpeg's fixed-point
/// coefficients and rounding.
fn ycck_to_cmy(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    const SCALEBITS: i32 = 16;
    const ONE_HALF: i32 = 1 << (SCALEBITS - 1);
    // FIX(x): x in 16.16 fixed point, rounded.
    const FIX_1_40200: i32 = 91881;
    const FIX_1_77200: i32 = 116_130;
    const FIX_0_71414: i32 = 46802;
    const FIX_0_34414: i32 = 22554;

    let y = i32::from(y);
    let cb = i32::from(cb) - 128;
    let cr = i32::from(cr) - 128;
    let r = y + ((FIX_1_40200 * cr + ONE_HALF) >> SCALEBITS);
    let g = y + ((-FIX_0_34414 * cb - FIX_0_71414 * cr + ONE_HALF) >> SCALEBITS);
    let b = y + ((FIX_1_77200 * cb + ONE_HALF) >> SCALEBITS);
    [r, g, b].map(|v| (255 - v).clamp(0, 255) as u8)
}

#[cfg(test)]
mod tests {
    // Not in the C++ suite. The expected pixels are libjpeg-turbo's.
    use super::*;

    fn read(path: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn tile() -> Vec<u8> {
        read("testdata/test/fixtures/image/tile.jpeg")
    }

    /// The fixture, with its Adobe marker's color transform set to
    /// `transform` (0: CMYK, 2: YCCK).
    fn adobe(transform: u8) -> Vec<u8> {
        let mut data = read("tests/fixtures/ycck.jpeg");
        let adobe = data.windows(5).position(|w| w == b"Adobe").unwrap();
        data[adobe + 11] = transform;
        data
    }

    #[test]
    fn opaque() {
        let image = decode_jpeg(&tile()).unwrap();
        assert!(image.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn grayscale_is_replicated() {
        let image = decode_jpeg(&read("tests/fixtures/gray.jpeg")).unwrap();
        assert_eq!(&image.data[..4], &[111, 111, 111, 255]);
        assert!(image.data.as_chunks::<4>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
    }

    // C++ keeps the C, M and Y of libjpeg's CMYK output as R, G and B.
    #[test]
    fn cmyk_keeps_cmy() {
        let image = decode_jpeg(&adobe(0)).unwrap();
        assert_eq!(&image.data[..4], &[5, 139, 130, 255]);
    }

    #[test]
    fn ycck_keeps_cmy() {
        let image = decode_jpeg(&adobe(2)).unwrap();
        assert_eq!(&image.data[..4], &[247, 255, 231, 255]);
    }

    #[test]
    fn malformed() {
        let data = tile();
        for bytes in [&data[..2], &data[..data.len() / 2], b"\xff\xd8\xff\xc0\x00\x02"] {
            let error = decode_jpeg(bytes).unwrap_err();
            assert!(error.message.starts_with("JPEG Reader: libjpeg could not read image: "), "{error}");
        }
    }

    #[test]
    fn corrupt_never_panics() {
        let data = adobe(2);
        for len in 0..data.len() {
            let _ = decode_jpeg(&data[..len]);
        }
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2000 {
            let mut corrupt = data.clone();
            for _ in 0..3 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = 2 + (seed as usize) % (corrupt.len() - 2);
                corrupt[at] = (seed >> 32) as u8;
            }
            let _ = decode_jpeg(&corrupt);
        }
    }
}
