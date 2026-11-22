//! The atlas of a background layer's patterns.

use std::rc::Rc;
use std::sync::Arc;

use etagere::{AllocId, BucketedAtlasAllocator, size2};
use geometry::Point;
use mln_gfx::gpu::TextureFormat;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_render::draw::{Resources, Texture};
use mln_render::layer::ImageStore;
use mln_style_types::image::{ImagePosition, Impl as ImageImpl, RectU16};
use mln_util::hash::HashMap;
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
    /// The style's image the pattern was packed from.
    image: Arc<ImageImpl>,
}

/// Pattern images packed into one texture, each with a padding that wraps
/// it.
pub(crate) struct PatternAtlas {
    allocator: BucketedAtlasAllocator,
    patterns: HashMap<String, Pattern>,
    atlas_image: PremultipliedImage,
    /// The texture that the atlas image uploads to.
    pub(crate) texture: Texture,
    dirty: bool,
}

impl PatternAtlas {
    /// An empty atlas, whose texture `res` makes.
    pub(crate) fn new(res: &mut Resources) -> Self {
        let texture = res.texture(|texture| {
            texture.set_format(TextureFormat::Rgba8Unorm);
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        });
        PatternAtlas {
            allocator: BucketedAtlasAllocator::new(size2(64, 64)),
            patterns: HashMap::default(),
            atlas_image: PremultipliedImage::new(Size::new(0, 0)),
            texture,
            dirty: true,
        }
    }

    /// Where the pattern `id` is in the atlas, if it holds it.
    #[must_use]
    pub(crate) fn pattern(&self, id: &str) -> Option<ImagePosition> {
        self.patterns.get(id).map(|pattern| pattern.position.clone())
    }

    /// Drops the patterns whose images `images` no longer has, or has
    /// replaced since they were packed.
    pub(crate) fn remove_stale(&mut self, images: &ImageStore) {
        let PatternAtlas { allocator, patterns, atlas_image, .. } = self;
        patterns.retain(|id, pattern| {
            let current = images.image(id).is_some_and(|image| Arc::ptr_eq(image, &pattern.image));
            if !current {
                let b = pattern.position.padded_rect;
                let _ = PremultipliedImage::clear(
                    atlas_image,
                    Point::new(u32::from(b.x), u32::from(b.y)),
                    Size::new(u32::from(b.w), u32::from(b.h)),
                );
                allocator.deallocate(pattern.alloc_id);
            }
            current
        });
    }

    /// Adds the image `id` of `images` as a pattern, unless the atlas holds
    /// it already or `images` has no such image.
    pub(crate) fn add(&mut self, id: &str, images: &ImageStore) {
        if !self.patterns.contains_key(id)
            && let Some(image) = images.image(id)
        {
            self.add_pattern(image);
        }
    }

    /// Adds `image` as a pattern, unless the atlas has no room for it.
    fn add_pattern(&mut self, image: &Arc<ImageImpl>) {
        let padded = |side: u32| u16::try_from(side).ok()?.checked_add(PADDING * 2);
        let (Some(width), Some(height)) = (padded(image.image.size.width), padded(image.image.size.height)) else {
            return;
        };
        let Some((alloc_id, bin)) = self.allocate(width, height) else {
            return;
        };

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

        let pattern = Pattern { alloc_id, position: ImagePosition::new(bin, image), image: Arc::clone(image) };
        self.patterns.insert(image.id.clone(), pattern);
    }

    #[must_use]
    fn pixel_size(&self) -> Size {
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

    /// Gives the texture the atlas image, unless the atlas is unchanged
    /// since the last call.
    pub(crate) fn upload(&mut self, res: &mut Resources) {
        if std::mem::take(&mut self.dirty)
            && let Some(mut texture) = res.texture_mut(&self.texture)
        {
            texture.set_image(Some(Rc::new(self.atlas_image.clone())));
        }
    }

    /// The size of the atlas image, which [`upload`](Self::upload) gives the
    /// texture.
    #[must_use]
    pub(crate) fn size(&self) -> Size {
        self.atlas_image.size
    }
}
