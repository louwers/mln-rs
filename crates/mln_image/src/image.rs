// Ports platform/default/src/mln/util/image.cpp

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;

/// The image formats `decode_image` recognizes, by their magic bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageType {
    Png,
    Webp,
    Jpeg,
}

/// The magic-byte dispatch of `mln::decodeImage`: the type of the image in
/// `data`, if it is one of the three.
pub fn image_type(data: &[u8]) -> Option<ImageType> {
    let read_uint = |offset: usize| -> u32 {
        let ptr = &data[offset..offset + 4];
        (u32::from(ptr[0]) << 24) | (u32::from(ptr[1]) << 16) | (u32::from(ptr[2]) << 8) | u32::from(ptr[3])
    };

    let size = data.len();

    if size >= 16 {
        let magic1 = read_uint(0x0);
        let magic2 = read_uint(0x8);
        // RIFF <xxxx = file size> WEBP
        if magic1 == 0x52494646 && magic2 == 0x57454250 {
            return Some(ImageType::Webp);
        }
    }

    if size >= 4 {
        let magic = read_uint(0x0);
        if magic == 0x89504E47 {
            return Some(ImageType::Png);
        }
    }

    if size >= 2 {
        let magic = ((u16::from(data[0]) << 8) | u16::from(data[1])) & 0xffff;
        if magic == 0xFFD8 {
            return Some(ImageType::Jpeg);
        }
    }

    None
}

/// The message of the error for data of no type `decode_image` recognizes.
pub const UNSUPPORTED: &str = "unsupported image type";

/// `mln::decodeImage(const std::string&)`: dispatches on the magic bytes.
/// Data of no type it recognizes fails with
/// [`ErrorKind::Unsupported`](mln_util::ErrorKind), and an image that
/// doesn't decode with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind).
pub fn decode_image(data: &[u8]) -> Result<PremultipliedImage, Error> {
    match image_type(data) {
        Some(ImageType::Webp) => Ok(crate::webp_reader::decode_webp(data)),
        Some(ImageType::Png) => crate::png_reader::decode_png(data),
        #[cfg(not(target_family = "wasm"))]
        Some(ImageType::Jpeg) => crate::jpeg_reader::decode_jpeg(data),
        // The browser build decodes JPEG with the browser (`decoder::decode`).
        #[cfg(target_family = "wasm")]
        Some(ImageType::Jpeg) => Err(Error::unsupported("JPEG Reader: JPEG is decoded by the browser")),
        None => Err(Error::unsupported(UNSUPPORTED)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported() {
        assert_eq!(decode_image(b"").unwrap_err(), Error::unsupported("unsupported image type"));
        assert_eq!(decode_image(b"GIF89a").unwrap_err(), Error::unsupported("unsupported image type"));
    }
}

#[cfg(test)]
mod fixture_tests {
    // Ports the fixture cases of test/util/image.test.cpp.
    use super::*;

    fn read_file(path: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    // TEST(Image, PNGReadNoProfile)
    // gtest: Image.PNGReadNoProfile
    #[test]
    fn png_read_no_profile() {
        let image = decode_image(&read_file("test/fixtures/image/no_profile.png")).unwrap();
        assert_eq!(&image.data[..4], &[128, 0, 0, 255]);
    }

    // TEST(Image, PNGReadNoProfileAlpha)
    // gtest: Image.PNGReadNoProfileAlpha
    #[test]
    fn png_read_no_profile_alpha() {
        let image = decode_image(&read_file("test/fixtures/image/no_profile_alpha.png")).unwrap();
        assert_eq!(&image.data[..4], &[64, 0, 0, 128]);
    }

    // TEST(Image, PNGReadProfile)
    // gtest: Image.PNGReadProfile
    #[test]
    fn png_read_profile() {
        let image = decode_image(&read_file("test/fixtures/image/profile.png")).unwrap();
        assert_eq!(&image.data[..4], &[128, 0, 0, 255]);
    }

    // TEST(Image, PNGReadProfileAlpha)
    // gtest: Image.PNGReadProfileAlpha
    #[test]
    fn png_read_profile_alpha() {
        let image = decode_image(&read_file("test/fixtures/image/profile_alpha.png")).unwrap();
        assert_eq!(&image.data[..4], &[64, 0, 0, 128]);
    }

    // TEST(Image, PNGTile)
    // gtest: Image.PNGTile
    #[test]
    fn png_tile() {
        let image = decode_image(&read_file("test/fixtures/image/tile.png")).unwrap();
        assert_eq!(image.size.width, 256);
        assert_eq!(image.size.height, 256);
    }

    // TEST(Image, JPEGTile)
    // gtest: Image.JPEGTile
    #[test]
    fn jpeg_tile() {
        let image = decode_image(&read_file("test/fixtures/image/tile.jpeg")).unwrap();
        assert_eq!(image.size.width, 256);
        assert_eq!(image.size.height, 256);
    }

    // TEST(Image, WebPTile)
    // gtest: Image.WebPTile
    #[test]
    fn webp_tile() {
        let image = decode_image(&read_file("test/fixtures/image/tile.webp")).unwrap();
        assert_eq!(image.size.width, 256);
        assert_eq!(image.size.height, 256);
    }

    // Not in the C++ suite: the lossless DEM tiles of the render tests
    // (cache-style.db `http://tiles/{z}-{x}-{y}.ocean.webp`, VP8L) decode
    // to their full size, opaque.
    #[test]
    fn webp_lossless_dem_tile() {
        let db = metrics_file("integration/tiles/1-0-0.ocean.webp");
        let image = decode_image(&db).unwrap();
        assert_eq!(image.size.width, 512);
        assert_eq!(image.size.height, 512);
        assert!(image.data.chunks_exact(4).all(|p| p[3] == 255));
    }

    fn metrics_file(path: &str) -> Vec<u8> {
        read_file(&format!("metrics/{path}"))
    }

    // Not in the C++ suite: corrupt data decodes to an empty image, as C++
    // (with a warning) instead of an error.
    #[test]
    fn webp_corrupt_is_empty() {
        let mut data = b"RIFF\x10\x00\x00\x00WEBPVP8L".to_vec();
        data.extend_from_slice(&[0u8; 8]);
        let image = decode_image(&data).unwrap();
        assert!(!image.valid());
    }
}
