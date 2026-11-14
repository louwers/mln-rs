use std::collections::HashMap;

use etagere::{AllocId, BucketedAtlasAllocator, size2};
use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::texture2d::{Texture2DPtr, upload_image};
use mln_style_types::image::{ImagePosition, Impl as ImageImpl, RectU16};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

/// When copied into the atlas texture, image data is padded by one pixel on
/// each side. Pattern images are padded with a copy of the image data
/// wrapped from the opposite side. This ensures the correct behavior of
/// `GL_LINEAR` texture sampling mode.
const PADDING: u16 = 1;

struct Pattern {
    alloc_id: AllocId,
    position: ImagePosition,
}

pub(crate) struct PatternAtlas {
    allocator: BucketedAtlasAllocator,
    patterns: HashMap<String, Pattern>,
    atlas_image: PremultipliedImage,
    atlas_texture_2d: Option<Texture2DPtr>,
    dirty: bool,
}

impl Default for PatternAtlas {
    fn default() -> Self {
        PatternAtlas::new()
    }
}

impl PatternAtlas {
    #[must_use]
    pub(crate) fn new() -> Self {
        PatternAtlas {
            allocator: BucketedAtlasAllocator::new(size2(64, 64)),
            patterns: HashMap::new(),
            atlas_image: PremultipliedImage::new(Size::new(0, 0)),
            atlas_texture_2d: None,
            dirty: true,
        }
    }

    #[must_use]
    pub(crate) fn get_pattern(&self, id: &str) -> Option<ImagePosition> {
        self.patterns.get(id).map(|pattern| pattern.position.clone())
    }

    pub(crate) fn add_pattern(&mut self, image: &ImageImpl) -> Option<ImagePosition> {
        if self.patterns.contains_key(&image.id) {
            return None;
        }
        let width = u16::try_from(image.image.size.width).ok()?.checked_add(PADDING * 2)?;
        let height = u16::try_from(image.image.size.height).ok()?.checked_add(PADDING * 2)?;

        let (alloc_id, bin) = self.allocate(width, height)?;

        let size = self.get_pixel_size();
        self.atlas_image.resize(size);

        let src = &image.image;
        let x = u32::from(bin.x) + u32::from(PADDING);
        let y = u32::from(bin.y) + u32::from(PADDING);
        let w = src.size.width;
        let h = src.size.height;

        let atlas = &mut self.atlas_image;
        let _ = PremultipliedImage::copy(src, atlas, Point::new(0, 0), Point::new(x, y), Size::new(w, h));

        // Add 1 pixel wrapped padding on each side of the image.
        let _ = PremultipliedImage::copy(
            src,
            atlas,
            Point::new(0, h.wrapping_sub(1)),
            Point::new(x, y - 1),
            Size::new(w, 1),
        ); // T
        let _ = PremultipliedImage::copy(src, atlas, Point::new(0, 0), Point::new(x, y + h), Size::new(w, 1)); // B
        let _ = PremultipliedImage::copy(
            src,
            atlas,
            Point::new(w.wrapping_sub(1), 0),
            Point::new(x - 1, y),
            Size::new(1, h),
        ); // L
        let _ = PremultipliedImage::copy(src, atlas, Point::new(0, 0), Point::new(x + w, y), Size::new(1, h)); // R

        self.dirty = true;

        let position = ImagePosition::new(bin, image, 0);
        self.patterns.insert(image.id.clone(), Pattern { alloc_id, position: position.clone() });
        Some(position)
    }

    pub(crate) fn remove_pattern(&mut self, id: &str) {
        if let Some(pattern) = self.patterns.remove(id) {
            // Clear pattern from the atlas image.
            let b = pattern.position.padded_rect;
            let _ = PremultipliedImage::clear(
                &mut self.atlas_image,
                Point::new(u32::from(b.x), u32::from(b.y)),
                Size::new(u32::from(b.w), u32::from(b.h)),
            );

            self.allocator.deallocate(pattern.alloc_id);
        }
    }

    #[must_use]
    pub(crate) fn get_pixel_size(&self) -> Size {
        let size = self.allocator.size();
        Size::new(size.width as u32, size.height as u32)
    }

    /// Allocates a `width` × `height` bin, growing the atlas until it fits:
    /// each step doubles the shorter side (the width when they're equal),
    /// or grows a side the bin is longer than to twice the bin's. The bin is
    /// at the corner of its allocation, which can be larger.
    fn allocate(&mut self, width: u16, height: u16) -> Option<(AllocId, RectU16)> {
        let (w, h) = (i32::from(width), i32::from(height));
        loop {
            if let Some(allocation) = self.allocator.allocate(size2(w, h)) {
                let min = allocation.rectangle.min;
                return Some((allocation.id, RectU16 { x: min.x as u16, y: min.y as u16, w: width, h: height }));
            }
            let size = self.allocator.size();
            let mut grown = size;
            if size.width <= size.height || w > size.width {
                grown.width = w.max(size.width) * 2;
            }
            if size.height < size.width || h > size.height {
                grown.height = h.max(size.height) * 2;
            }
            if grown.width >= i32::from(u16::MAX) || grown.height >= i32::from(u16::MAX) {
                return None;
            }
            self.allocator.grow(grown);
        }
    }

    /// Uploads the atlas image to its texture.
    ///
    /// Deviation: when the atlas grew since the texture was created, the
    /// texture is created again. C++ uploads the larger image into the old
    /// texture (`webgpu::Texture2D` never resizes an existing texture), which
    /// Dawn reports as a validation error and wgpu treats as fatal.
    pub(crate) fn upload(&mut self, upload_pass: &dyn UploadPass) {
        if let Some(texture) = &self.atlas_texture_2d {
            let texture_size = texture.borrow().get_size();
            if texture_size != self.atlas_image.size && !texture_size.is_empty() {
                self.atlas_texture_2d = None;
            }
        }
        match &self.atlas_texture_2d {
            None => {
                let texture = upload_pass.get_context().create_texture_2d();
                upload_image(&mut *texture.borrow_mut(), &self.atlas_image);
                self.atlas_texture_2d = Some(texture);
            }
            Some(texture) => {
                if self.dirty {
                    upload_image(&mut *texture.borrow_mut(), &self.atlas_image);
                }
            }
        }
        self.dirty = false;
    }

    #[must_use]
    pub(crate) fn texture(&self) -> Option<&Texture2DPtr> {
        self.atlas_texture_2d.as_ref()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn get_atlas_image_for_test(&self) -> &PremultipliedImage {
        &self.atlas_image
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}
