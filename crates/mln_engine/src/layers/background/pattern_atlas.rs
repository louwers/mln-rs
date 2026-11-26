use etagere::{AllocId, BucketedAtlasAllocator, size2};
use geometry::Point;
use mln_gfx::gpu::{Gpu, TextureFormat};
use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::HashMap;
use mln_style::image::Impl as ImageImpl;
use mln_style::size::Size;

use crate::draw::context::Context;
use crate::draw::store::SharedTexture;
use crate::draw::texture_size;
use crate::text::atlas::{ImagePosition, RectU16};

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
    texture: Option<SharedTexture>,
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
            patterns: HashMap::default(),
            atlas_image: PremultipliedImage::new(Size::new(0, 0)),
            texture: None,
            dirty: true,
        }
    }

    #[must_use]
    pub(crate) fn pattern(&self, id: &str) -> Option<ImagePosition> {
        self.patterns.get(id).map(|pattern| pattern.position.clone())
    }

    pub(crate) fn add_pattern(&mut self, image: &ImageImpl) -> Option<ImagePosition> {
        if self.patterns.contains_key(&image.id) {
            return None;
        }
        let width = u16::try_from(image.image.size.width).ok()?.checked_add(PADDING * 2)?;
        let height = u16::try_from(image.image.size.height).ok()?.checked_add(PADDING * 2)?;

        let (alloc_id, bin) = self.allocate(width, height)?;

        let size = self.pixel_size();
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

        let position = ImagePosition::new(bin, image);
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
    pub(crate) fn pixel_size(&self) -> Size {
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

    /// Uploads the atlas image to its texture, which is made again when the
    /// atlas grew since.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        let texture = match &self.texture {
            None => {
                let texture = context.add_texture_with(|texture| texture.set_format(TextureFormat::Rgba8Unorm));
                &*self.texture.insert(texture)
            }
            Some(texture) if self.dirty => texture,
            Some(_) => return,
        };
        let image = &self.atlas_image;
        if let Err(error) = context.upload_texture_data(gpu, texture, &image.data, texture_size(image.size)) {
            context.fail(format!("Uploading the pattern atlas failed: {error}"));
        }
        self.dirty = false;
    }

    /// The atlas's texture, once `upload` made it.
    #[must_use]
    pub(crate) fn texture(&self) -> Option<&SharedTexture> {
        self.texture.as_ref()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn atlas_image(&self) -> &PremultipliedImage {
        &self.atlas_image
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}
