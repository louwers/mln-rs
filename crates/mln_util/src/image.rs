// Ports include/mln/util/image.hpp
//
// `decodeImage` and `encodePNG`, declared in image.hpp, are implemented by the
// platform (platform/default/src/mln/util/{image,png_reader,png_writer}.cpp);
// their port lives in //crates/mln_image so that this file
// stays dependency-free.

use std::fmt;
use std::marker::PhantomData;

use geometry::Point;
use serde::{Deserialize, Serialize};

use crate::size::Size;

/// `mln::ImageAlphaMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageAlphaMode {
    Unassociated,
    Premultiplied,
    /// Alpha-channel only.
    Exclusive,
}

/// The `ImageAlphaMode Mode` template parameter of `mln::Image<Mode>`.
pub trait AlphaMode {
    const MODE: ImageAlphaMode;
    /// `Image::channels`: 1 for `Exclusive`, 4 otherwise.
    const CHANNELS: usize;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Unassociated;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Premultiplied;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Exclusive;

impl AlphaMode for Unassociated {
    const MODE: ImageAlphaMode = ImageAlphaMode::Unassociated;
    const CHANNELS: usize = 4;
}
impl AlphaMode for Premultiplied {
    const MODE: ImageAlphaMode = ImageAlphaMode::Premultiplied;
    const CHANNELS: usize = 4;
}
impl AlphaMode for Exclusive {
    const MODE: ImageAlphaMode = ImageAlphaMode::Exclusive;
    const CHANNELS: usize = 1;
}

/// The exceptions `Image` throws in C++ (`std::invalid_argument`,
/// `std::out_of_range`), with the same messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    InvalidArgument(&'static str),
    OutOfRange(&'static str),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::InvalidArgument(msg) | ImageError::OutOfRange(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ImageError {}

/// `mln::Image<Mode>`: `size` plus `channels * width * height` bytes.
///
/// C++ holds the pixels in a nullable `std::unique_ptr<uint8_t[]>`; here an
/// empty `data` plays the role of `nullptr` (a default-constructed or
/// moved-from image). C++ images are move-only with an explicit `clone()`;
/// here `Clone` is that `clone()`.
///
/// Port-only: serde writes the pixels as a payload (serialization/payload.rs)
/// and reads back only images whose pixels fill their size.
#[derive(Serialize, Deserialize)]
#[serde(bound = "", try_from = "ImageParts")]
pub struct Image<M: AlphaMode> {
    pub size: Size,
    #[serde(with = "crate::serialization::payload::bytes")]
    pub data: Vec<u8>,
    #[serde(skip)]
    _mode: PhantomData<M>,
}

/// What `Image` is read from.
#[derive(Deserialize)]
struct ImageParts {
    size: Size,
    #[serde(with = "crate::serialization::payload::bytes")]
    data: Vec<u8>,
}

impl<M: AlphaMode> TryFrom<ImageParts> for Image<M> {
    type Error = String;

    fn try_from(ImageParts { size, data }: ImageParts) -> Result<Self, String> {
        // In u64: on wasm32 `bytes()` would wrap for a large size.
        let bytes =
            u64::from(size.width).checked_mul(u64::from(size.height)).and_then(|n| n.checked_mul(M::CHANNELS as u64));
        if bytes != Some(data.len() as u64) {
            return Err(format!("{} bytes for a {}x{} image", data.len(), size.width, size.height));
        }
        Ok(Image::from_vec(size, data))
    }
}

pub type UnassociatedImage = Image<Unassociated>;
pub type PremultipliedImage = Image<Premultiplied>;
pub type AlphaImage = Image<Exclusive>;

impl<M: AlphaMode> Image<M> {
    pub const CHANNELS: usize = M::CHANNELS;

    /// `Image(Size)`: zero-initialized pixels (`std::make_unique<uint8_t[]>`
    /// value-initializes).
    pub fn new(size: Size) -> Self {
        let bytes = M::CHANNELS * size.width as usize * size.height as usize;
        Image { size, data: vec![0; bytes], _mode: PhantomData }
    }

    /// `Image(Size, const uint8_t* srcData, std::size_t srcLength)`.
    pub fn from_slice(size: Size, src: &[u8]) -> Result<Self, ImageError> {
        let mut image = Image { size, data: Vec::new(), _mode: PhantomData };
        if src.len() != image.bytes() {
            return Err(ImageError::InvalidArgument("mismatched image size"));
        }
        if image.bytes() != 0 {
            image.data = src.to_vec();
        }
        Ok(image)
    }

    /// `Image(Size, std::unique_ptr<uint8_t[]>)`: takes the buffer as is (no
    /// size check, as in C++).
    pub fn from_vec(size: Size, data: Vec<u8>) -> Self {
        Image { size, data, _mode: PhantomData }
    }

    /// Splits the image into its size and pixel buffer (the C++ code moves
    /// `data` out of one image type into another).
    pub fn into_parts(self) -> (Size, Vec<u8>) {
        (self.size, self.data)
    }

    pub fn valid(&self) -> bool {
        !self.size.is_empty() && !self.data.is_empty()
    }

    pub fn stride(&self) -> usize {
        M::CHANNELS * self.size.width as usize
    }

    pub fn bytes(&self) -> usize {
        self.stride() * self.size.height as usize
    }

    pub fn fill(&mut self, value: u8) {
        if self.valid() {
            let bytes = self.bytes();
            self.data[..bytes].fill(value);
        }
    }

    pub fn resize(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        let mut new_image = Image::<M>::new(size);
        if !size.is_empty() && self.valid() {
            new_image.fill(0);
            let copy_size =
                Size::new(std::cmp::min(self.size.width, size.width), std::cmp::min(self.size.height, size.height));
            // Both rectangles are in bounds by construction, as in C++.
            Self::copy(self, &mut new_image, Point::new(0, 0), Point::new(0, 0), copy_size)
                .expect("resize copy is in bounds");
        }
        *self = new_image;
    }

    /// Clears the rect area specified by `pt` and `size` from `dst_img`.
    pub fn clear(dst_img: &mut Self, pt: Point<u32>, size: Size) -> Result<(), ImageError> {
        if size.is_empty() {
            return Ok(());
        }

        if !dst_img.valid() {
            return Err(ImageError::InvalidArgument("invalid destination for image clear"));
        }

        if size.width > dst_img.size.width
            || size.height > dst_img.size.height
            || pt.x > dst_img.size.width - size.width
            || pt.y > dst_img.size.height - size.height
        {
            return Err(ImageError::OutOfRange("out of range destination coordinates for image clear"));
        }

        let stride = dst_img.stride();
        for y in 0..size.height {
            let dst_offset = (pt.y + y) as usize * stride + pt.x as usize * M::CHANNELS;
            dst_img.data[dst_offset..dst_offset + size.width as usize * M::CHANNELS].fill(0);
        }
        Ok(())
    }

    /// Copy image data within `rect` from `src` to the rectangle of the same
    /// size at `pt` in `dst`. If the specified bounds exceed the bounds of the
    /// source or destination, return `ImageError::OutOfRange` (C++ throws
    /// `std::out_of_range`). Borrowing rules rule out copying within a single
    /// image (a C++ `assert`).
    pub fn copy(
        src_img: &Self,
        dst_img: &mut Self,
        src_pt: Point<u32>,
        dst_pt: Point<u32>,
        size: Size,
    ) -> Result<(), ImageError> {
        if size.is_empty() {
            return Ok(());
        }

        if !src_img.valid() {
            return Err(ImageError::InvalidArgument("invalid source for image copy"));
        }

        if !dst_img.valid() {
            return Err(ImageError::InvalidArgument("invalid destination for image copy"));
        }

        if size.width > src_img.size.width
            || size.height > src_img.size.height
            || src_pt.x > src_img.size.width - size.width
            || src_pt.y > src_img.size.height - size.height
        {
            return Err(ImageError::OutOfRange("out of range source coordinates for image copy"));
        }

        if size.width > dst_img.size.width
            || size.height > dst_img.size.height
            || dst_pt.x > dst_img.size.width - size.width
            || dst_pt.y > dst_img.size.height - size.height
        {
            return Err(ImageError::OutOfRange("out of range destination coordinates for image copy"));
        }

        let src_stride = src_img.stride();
        let dst_stride = dst_img.stride();
        let row = size.width as usize * M::CHANNELS;
        for y in 0..size.height {
            let src_offset = (src_pt.y + y) as usize * src_stride + src_pt.x as usize * M::CHANNELS;
            let dst_offset = (dst_pt.y + y) as usize * dst_stride + dst_pt.x as usize * M::CHANNELS;
            dst_img.data[dst_offset..dst_offset + row].copy_from_slice(&src_img.data[src_offset..src_offset + row]);
        }
        Ok(())
    }

    fn pixels(&self) -> &[u8] {
        let bytes = self.bytes();
        self.data.get(..bytes).unwrap_or(&self.data)
    }
}

impl<M: AlphaMode> Default for Image<M> {
    fn default() -> Self {
        Image { size: Size::default(), data: Vec::new(), _mode: PhantomData }
    }
}

impl<M: AlphaMode> Clone for Image<M> {
    fn clone(&self) -> Self {
        Image { size: self.size, data: self.data.clone(), _mode: PhantomData }
    }
}

/// `operator==` compares the pixel bytes only (`std::equal` over both
/// `[data, data + bytes())` ranges).
impl<M: AlphaMode> PartialEq for Image<M> {
    fn eq(&self, other: &Self) -> bool {
        self.pixels() == other.pixels()
    }
}

impl<M: AlphaMode> Eq for Image<M> {}

impl<M: AlphaMode> fmt::Debug for Image<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("mode", &M::MODE)
            .field("size", &self.size)
            .field("data.len()", &self.data.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports TEST(Image, Resize) from test/util/image.test.cpp.
    // gtest: Image.Resize
    #[test]
    fn resize() {
        let mut image = AlphaImage::new(Size::new(0, 0));

        image.resize(Size::new(1, 1));
        assert_eq!(image.size, Size::new(1, 1));

        image.fill(100);
        image.resize(Size::new(2, 1));
        assert_eq!(image.size, Size::new(2, 1));
        assert_eq!(image.data[0], 100);
        assert_eq!(image.data[1], 0);

        image.resize(Size::new(0, 0));
        assert_eq!(image.size, Size::new(0, 0));
    }

    // Ports TEST(Image, Copy) from test/util/image.test.cpp.
    // gtest: Image.Copy
    #[test]
    fn copy() {
        let src5 = PremultipliedImage::new(Size::new(5, 5));
        let mut dst5 = PremultipliedImage::new(Size::new(5, 5));
        let src10 = PremultipliedImage::new(Size::new(10, 10));
        let mut dst10 = PremultipliedImage::new(Size::new(10, 10));
        let p = |x, y| Point::new(x, y);
        let s = Size::new;
        let out_of_range = |r: Result<(), ImageError>| matches!(r, Err(ImageError::OutOfRange(_)));

        assert!(out_of_range(PremultipliedImage::copy(&src5, &mut dst10, p(0, 0), p(0, 0), s(6, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src5, &mut dst10, p(0, 0), p(0, 0), s(1, 6))));
        assert!(out_of_range(PremultipliedImage::copy(&src5, &mut dst10, p(1, 1), p(0, 0), s(5, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src5, &mut dst10, p(1, 1), p(0, 0), s(1, 5))));

        assert!(out_of_range(PremultipliedImage::copy(&src10, &mut dst5, p(0, 0), p(0, 0), s(6, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10, &mut dst5, p(0, 0), p(0, 0), s(1, 6))));
        assert!(out_of_range(PremultipliedImage::copy(&src10, &mut dst5, p(0, 0), p(1, 1), s(5, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10, &mut dst5, p(0, 0), p(1, 1), s(1, 5))));

        let max = u32::MAX;

        let src10b = src10.clone();
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(max, 0), p(0, 0), s(1, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, max), p(0, 0), s(1, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, 0), p(max, 0), s(1, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, 0), p(0, max), s(1, 1))));

        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(1, 0), p(0, 0), s(max, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, 1), p(0, 0), s(1, max))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, 0), p(1, 0), s(max, 1))));
        assert!(out_of_range(PremultipliedImage::copy(&src10b, &mut dst10, p(0, 0), p(0, 1), s(1, max))));
    }

    // Ports TEST(Image, Move): a Rust move leaves nothing behind to inspect;
    // `std::mem::take` is the equivalent of the C++ moved-from state.
    // gtest: Image.Move
    #[test]
    fn move_() {
        let mut rgba = UnassociatedImage::new(Size::new(1, 1));
        rgba.data.copy_from_slice(&[255, 254, 253, 128]);

        let moved = std::mem::take(&mut rgba);

        assert_eq!(rgba.size.width, 0);
        assert!(rgba.data.is_empty());
        assert_eq!(moved.data[1], 254);
        assert_eq!(moved.size.width, 1);
    }

    // Not in the C++ suite: the remaining Image members.
    #[test]
    fn construct_clear_equality() {
        let image = PremultipliedImage::new(Size::new(2, 3));
        assert_eq!((image.bytes(), image.stride()), (24, 8));
        assert!(image.valid() && image.data.iter().all(|&b| b == 0));
        assert_eq!(AlphaImage::new(Size::new(2, 3)).bytes(), 6);
        assert!(!PremultipliedImage::default().valid());
        assert!(!PremultipliedImage::new(Size::new(0, 3)).valid());
        assert_eq!(
            PremultipliedImage::from_slice(Size::new(1, 1), &[1, 2, 3]),
            Err(ImageError::InvalidArgument("mismatched image size"))
        );
        assert_eq!(PremultipliedImage::from_slice(Size::new(1, 1), &[1, 2, 3, 4]).unwrap().data, vec![1, 2, 3, 4]);

        let mut image = PremultipliedImage::new(Size::new(3, 3));
        image.fill(7);
        PremultipliedImage::clear(&mut image, Point::new(1, 1), Size::new(2, 2)).unwrap();
        assert_eq!(&image.data[0..16], &[7; 16]);
        assert_eq!(&image.data[16..24], &[0; 8]);
        assert_eq!(&image.data[24..28], &[7; 4]);
        assert_eq!(&image.data[28..36], &[0; 8]);
        assert!(matches!(
            PremultipliedImage::clear(&mut image, Point::new(2, 2), Size::new(2, 2)),
            Err(ImageError::OutOfRange(_))
        ));
        let mut empty = PremultipliedImage::default();
        assert!(matches!(
            PremultipliedImage::clear(&mut empty, Point::new(0, 0), Size::new(1, 1)),
            Err(ImageError::InvalidArgument(_))
        ));
        PremultipliedImage::clear(&mut empty, Point::new(0, 0), Size::new(0, 1)).unwrap();

        let mut src = UnassociatedImage::new(Size::new(4, 4));
        let mut dst = UnassociatedImage::new(Size::new(4, 4));
        src.fill(0xFF);
        UnassociatedImage::copy(&src, &mut dst, Point::new(0, 0), Point::new(1, 1), Size::new(2, 2)).unwrap();
        for y in 0..4usize {
            for x in 0..4usize {
                let inside = (1..3).contains(&x) && (1..3).contains(&y);
                let px = &dst.data[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
                assert_eq!(px, if inside { &[0xFF; 4] } else { &[0; 4] }, "({x}, {y})");
            }
        }

        let a = PremultipliedImage::from_vec(Size::new(1, 1), vec![1, 2, 3, 4]);
        assert_eq!(a, PremultipliedImage::from_vec(Size::new(1, 1), vec![1, 2, 3, 4]));
        assert_ne!(a, PremultipliedImage::from_vec(Size::new(1, 1), vec![1, 2, 3, 5]));
    }
}
