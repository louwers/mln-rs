use std::collections::HashMap;

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::texture2d::{Texture2DPtr, upload_image};
use mln_style_types::image::{ImagePosition, Impl as ImageImpl, RectU16};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use shelf_pack::{BinRef, ShelfPack, ShelfPackOptions};

/// When copied into the atlas texture, image data is padded by one pixel on
/// each side. Pattern images are padded with a copy of the image data
/// wrapped from the opposite side. This ensures the correct behavior of
/// `GL_LINEAR` texture sampling mode.
const PADDING: u16 = 1;

fn shelf_pack_options() -> ShelfPackOptions {
    ShelfPackOptions { auto_resize: true }
}

struct Pattern {
    bin: BinRef,
    position: ImagePosition,
}

pub(crate) struct PatternAtlas {
    shelf_pack: ShelfPack,
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
            shelf_pack: ShelfPack::new(64, 64, shelf_pack_options()),
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
        let width = (image.image.size.width as u16).wrapping_add(PADDING * 2);
        let height = (image.image.size.height as u16).wrapping_add(PADDING * 2);

        let bin = self.shelf_pack.pack_one(-1, i32::from(width), i32::from(height))?;

        let size = self.get_pixel_size();
        self.atlas_image.resize(size);

        let src = &image.image;
        let (bx, by, bw, bh) = {
            let b = self.shelf_pack.bin(bin);
            (b.x, b.y, b.w, b.h)
        };

        let x = bx as u32 + u32::from(PADDING);
        let y = by as u32 + u32::from(PADDING);
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

        let position = ImagePosition::new(RectU16 { x: bx as u16, y: by as u16, w: bw as u16, h: bh as u16 }, image, 0);
        self.patterns.insert(image.id.clone(), Pattern { bin, position: position.clone() });
        Some(position)
    }

    pub(crate) fn remove_pattern(&mut self, id: &str) {
        if let Some(pattern) = self.patterns.remove(id) {
            // Clear pattern from the atlas image.
            let b = self.shelf_pack.bin(pattern.bin);
            let (x, y, w, h) = (b.x as u32, b.y as u32, b.w as u32, b.h as u32);
            let _ = PremultipliedImage::clear(&mut self.atlas_image, Point::new(x, y), Size::new(w, h));

            self.shelf_pack.unref(pattern.bin);
        }
    }

    #[must_use]
    pub(crate) fn get_pixel_size(&self) -> Size {
        Size::new(self.shelf_pack.width() as u32, self.shelf_pack.height() as u32)
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
