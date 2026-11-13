// Encodes PNGs without a PNG library, exactly like C++: unpremultiply, one
// IDAT chunk with filter type 0 on every scanline, zlib-compressed by
// `util::compress`.

use mln_util::compression::compress;
use mln_util::image::PremultipliedImage;
use mln_util::premultiply::unpremultiply;

fn network_byte_u32(value: u32) -> [u8; 4] {
    value.to_be_bytes()
}

fn add_chunk(png: &mut Vec<u8>, type_: [u8; 4], data: &[u8]) {
    // Checksum encompasses type + data
    let mut checksum = flate2::Crc::new();
    checksum.update(&type_);
    checksum.update(data);

    let size = data.len() as u32;
    png.reserve(4 /* length */ + 4 /* type */ + data.len() + 4 /* CRC */);
    png.extend_from_slice(&network_byte_u32(size));
    png.extend_from_slice(&type_);
    png.extend_from_slice(data);
    png.extend_from_slice(&network_byte_u32(checksum.sum()));
}

#[must_use]
pub fn encode_png(pre: &PremultipliedImage) -> Vec<u8> {
    // Make copy of the image so that we can unpremultiply it.
    let src = unpremultiply(pre.clone());

    // PNG magic bytes
    let preamble: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

    // IHDR chunk for our RGBA image.
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&network_byte_u32(src.size.width)); // width
    ihdr[4..8].copy_from_slice(&network_byte_u32(src.size.height)); // height
    ihdr[8] = 8; // bit depth == 8 bits
    ihdr[9] = 6; // color type == RGBA
    ihdr[10] = 0; // compression method == deflate
    ihdr[11] = 0; // filter method == default
    ihdr[12] = 0; // interlace method == none

    // Prepare the (compressed) data chunk.
    let stride = src.stride();
    let mut idat = Vec::with_capacity((stride + 1) * src.size.height as usize);
    for y in 0..src.size.height as usize {
        // Every scanline needs to be prefixed with one byte that indicates the filter type.
        idat.push(0); // filter type 0
        idat.extend_from_slice(&src.data[y * stride..(y + 1) * stride]);
    }
    // C++ throws "failed to initialize deflate" if zlib cannot start; an
    // in-memory flate2 encoder cannot fail.
    let idat = compress(&idat).expect("in-memory deflate");

    // Assemble the PNG.
    let mut png = Vec::with_capacity(8 + (12 + 13) + (12 + idat.len()) + 12);
    png.extend_from_slice(&preamble);
    add_chunk(&mut png, *b"IHDR", &ihdr);
    add_chunk(&mut png, *b"IDAT", &idat);
    add_chunk(&mut png, *b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    // Ports the PNG cases of test/util/image.test.cpp that need no fixtures.
    use mln_util::size::Size;

    use super::*;
    use crate::image::decode_image;

    // TEST(Image, PNGRoundTrip)
    // gtest: Image.PNGRoundTrip
    #[test]
    fn png_round_trip() {
        let mut rgba = PremultipliedImage::new(Size::new(1, 1));
        rgba.data.copy_from_slice(&[128, 0, 0, 255]);

        let image = decode_image(&encode_png(&rgba)).unwrap();
        assert_eq!(&image.data[..], &[128, 0, 0, 255]);
    }

    // TEST(Image, PNGRoundTripAlpha)
    // gtest: Image.PNGRoundTripAlpha
    #[test]
    fn png_round_trip_alpha() {
        let mut rgba = PremultipliedImage::new(Size::new(1, 1));
        rgba.data.copy_from_slice(&[128, 0, 0, 128]);

        let image = decode_image(&encode_png(&rgba)).unwrap();
        assert_eq!(&image.data[..], &[128, 0, 0, 128]);
    }

    // Not in the C++ suite: a larger image with every alpha value survives
    // encode -> decode as premultiply(unpremultiply(x)).
    #[test]
    fn png_round_trip_large() {
        let size = Size::new(256, 3);
        let mut data = Vec::new();
        for y in 0..3u32 {
            for x in 0..256u32 {
                let a = x as u8;
                let c = ((x * (y + 1)) % 256) as u8;
                let c = c.min(a); // a valid premultiplied pixel
                data.extend_from_slice(&[c, c / 2, 0, a]);
            }
        }
        let pre = PremultipliedImage::from_vec(size, data);
        let png = encode_png(&pre);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
        let decoded = decode_image(&png).unwrap();
        assert_eq!(decoded.size, size);
        let expected = mln_util::premultiply::premultiply(unpremultiply(pre.clone()));
        assert_eq!(decoded, expected);
    }

    #[test]
    fn png_reader_errors() {
        assert_eq!(crate::png_reader::decode_png(b"\x89PNG").unwrap_err().message, "PNG reader: Could not read image");
        assert_eq!(
            crate::png_reader::decode_png(b"\x89PNGxxxxxxxx").unwrap_err().message,
            "File or stream is not a png"
        );
        let truncated = &encode_png(&PremultipliedImage::new(Size::new(4, 4)))[..40];
        assert!(
            crate::png_reader::decode_png(truncated).unwrap_err().message.starts_with("failed to read invalid png: '")
        );
    }

    // Decodes every expected.png of the M1 render tests: they are 8-bit RGBA
    // images of the declared size, and re-encoding them is lossless.
    #[test]
    fn render_test_expectations_round_trip() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/metrics/integration/render-tests");
        for test in [
            "background-color/default",
            "background-color/literal",
            "background-color/function",
            "background-color/colorSpace-hcl",
            "background-color/colorSpace-lab",
            "background-color/transition",
            "background-opacity/color",
            "background-opacity/overlay",
            "background-visibility/none",
            "background-visibility/visible",
        ] {
            let bytes = std::fs::read(base.join(test).join("expected.png")).unwrap();
            let image = decode_image(&bytes).unwrap_or_else(|e| panic!("{test}: {e}"));
            assert!(image.valid(), "{test}");
            let again = decode_image(&encode_png(&image)).unwrap();
            assert_eq!(again.size, image.size, "{test}");
            assert_eq!(again, mln_util::premultiply::premultiply(unpremultiply(image.clone())), "{test}");
        }
    }
}
