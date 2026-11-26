// A page of the dynamic texture atlas (dynamic_texture_atlas.rs), which owns
// it on the render thread. The page's texture in the context's store is made
// in its first `upload`; the tiles whose layouts are on the page find it by
// the page's id once it is (`TileAtlasTextures::resolve`).

use std::num::NonZeroU32;
use std::sync::Arc;

use etagere::{AllocId, AtlasAllocator, size2};
use mln_gfx::gpu::Gpu;
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_style::bitmap::AlphaImage;
use mln_style::hash::HashMap;
use mln_style::size::Size;

use crate::draw::context::Context;
use crate::draw::store::SharedTexture;
use crate::draw::texture_size;
use crate::text::atlas::RectU16;
use crate::text::glyph::Glyph;

/// Identifies a bin within a dynamic texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DynamicTextureBinId(NonZeroU32);

impl DynamicTextureBinId {
    fn new(id: u32) -> Option<Self> {
        NonZeroU32::new(id).map(Self)
    }

    /// The id of a page's first bin.
    pub(crate) const fn first() -> Self {
        Self(NonZeroU32::MIN)
    }

    fn next(self) -> Option<Self> {
        self.0.get().checked_add(1).and_then(Self::new)
    }
}

/// A bin of a dynamic texture.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextureHandle {
    id: DynamicTextureBinId,
    rectangle: RectU16,
    needs_upload: bool,
}

impl TextureHandle {
    #[must_use]
    pub(crate) fn id(&self) -> DynamicTextureBinId {
        self.id
    }

    #[must_use]
    pub(crate) fn rectangle(&self) -> &RectU16 {
        &self.rectangle
    }

    #[must_use]
    pub(crate) fn is_upload_needed(&self) -> bool {
        self.needs_upload
    }
}

/// Handles compare by id.
impl PartialEq for TextureHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TextureHandle {}

/// An allocated bin and the handles that share it.
struct Bin {
    alloc_id: AllocId,
    rectangle: RectU16,
    refcount: u32,
}

/// The pixels queued for a bin.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QueuedPixels {
    /// The bin's pixels, row by row.
    Bytes(Vec<u8>),
    /// A glyph's bitmap, `padding` pixels in from the bin's corner, with the
    /// rest of the bin clear. The upload pads it, so that the queue holds no
    /// copy of a glyph the atlas keeps alive anyway.
    Glyph { glyph: Arc<Glyph>, padding: u16 },
}

/// A texture atlas page of images or glyphs: its bins, and the pixels queued
/// for the frame's next upload.
pub(crate) struct DynamicTexture {
    size: Size,
    pixel_format: TexturePixelType,
    allocator: AtlasAllocator,
    /// The allocated bins, by handle id.
    bins: HashMap<DynamicTextureBinId, Bin>,
    /// The highest handle id given out or asked for; a new bin without an id
    /// takes the next one.
    max_id: Option<DynamicTextureBinId>,
    /// The images queued for the frame's next upload, keyed by handle id.
    images_to_upload: HashMap<DynamicTextureBinId, (RectU16, QueuedPixels)>,
    /// The GPU texture, from the first upload.
    texture: Option<SharedTexture>,
}

impl std::fmt::Debug for DynamicTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicTexture").field("size", &self.size).finish_non_exhaustive()
    }
}

fn pixel_stride(pixel_type: TexturePixelType) -> usize {
    match pixel_type {
        TexturePixelType::Alpha | TexturePixelType::Luminance => 1,
        TexturePixelType::RGBA | TexturePixelType::Stencil | TexturePixelType::Depth => 4,
    }
}

/// Writes into `padded` the pixels of a bin of `rect`'s size that holds
/// `bitmap` `padding` pixels in from its corner, the rest clear.
fn pad_glyph(bitmap: &AlphaImage, rect: RectU16, padding: u16, padded: &mut Vec<u8>) {
    let width = usize::from(rect.w);
    let padding = usize::from(padding);
    let bitmap_width = bitmap.size.width as usize;
    padded.clear();
    padded.resize(width * usize::from(rect.h), 0);
    if width == 0 || bitmap_width == 0 {
        return;
    }
    for (row, source) in padded.chunks_exact_mut(width).skip(padding).zip(bitmap.data.chunks_exact(bitmap_width)) {
        if let Some(target) = row.get_mut(padding..padding + bitmap_width) {
            target.copy_from_slice(source);
        }
    }
}

impl DynamicTexture {
    pub(crate) fn new(size: Size, pixel_type: TexturePixelType) -> Self {
        DynamicTexture {
            size,
            pixel_format: pixel_type,
            allocator: AtlasAllocator::new(size2(size.width as i32, size.height as i32)),
            bins: HashMap::default(),
            max_id: None,
            images_to_upload: HashMap::default(),
            texture: None,
        }
    }

    /// The page's texture in the context's store, from the first upload.
    pub(crate) fn texture(&self) -> Option<&SharedTexture> {
        self.texture.as_ref()
    }

    pub(crate) fn pixel_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    /// The rectangle of the bin with id `id`, if it has one.
    pub(crate) fn bin(&self, id: DynamicTextureBinId) -> Option<RectU16> {
        self.bins.get(&id).map(|bin| bin.rectangle)
    }

    /// Reserves a bin of `size`, or another reference to the bin with `id` if
    /// it has one. `None` asks for a new bin with a fresh id. The handle needs
    /// an upload when its bin is new.
    pub(crate) fn reserve_size(&mut self, size: Size, id: Option<DynamicTextureBinId>) -> Option<TextureHandle> {
        if let Some(id) = id
            && let Some(bin) = self.bins.get_mut(&id)
        {
            bin.refcount += 1;
            return Some(TextureHandle { id, rectangle: bin.rectangle, needs_upload: false });
        }
        let id = match id {
            Some(id) => id,
            None => match self.max_id {
                Some(max_id) => max_id.next()?,
                None => DynamicTextureBinId::first(),
            },
        };
        let width = u16::try_from(size.width).ok()?;
        let height = u16::try_from(size.height).ok()?;
        let allocation = self.allocator.allocate(size2(i32::from(width), i32::from(height)))?;
        self.max_id = Some(self.max_id.map_or(id, |max_id| max_id.max(id)));
        // The allocation can be taller than asked for; the bin is the size
        // asked for, at its corner.
        let min = allocation.rectangle.min;
        let rectangle = RectU16 { x: min.x as u16, y: min.y as u16, w: width, h: height };
        self.bins.insert(id, Bin { alloc_id: allocation.id, rectangle, refcount: 1 });
        Some(TextureHandle { id, rectangle, needs_upload: true })
    }

    pub(crate) fn add_image(
        &mut self,
        pixel_data: Vec<u8>,
        image_size: Size,
        id: Option<DynamicTextureBinId>,
    ) -> Option<TextureHandle> {
        let mut tex_handle = self.reserve_size(image_size, id)?;
        if tex_handle.is_upload_needed() {
            self.upload_image(pixel_data, &mut tex_handle);
        }
        Some(tex_handle)
    }

    /// Queues `pixel_data`, the pixels of the handle's rectangle, for the
    /// frame's next upload.
    pub(crate) fn upload_image(&mut self, pixel_data: Vec<u8>, tex_handle: &mut TextureHandle) {
        let rect = tex_handle.rectangle;
        debug_assert_eq!(pixel_data.len(), usize::from(rect.w) * usize::from(rect.h) * pixel_stride(self.pixel_format));
        self.queue(tex_handle, QueuedPixels::Bytes(pixel_data));
    }

    /// Queues `glyph`'s bitmap, `padding` pixels in from the corner of the
    /// handle's rectangle, for the frame's next upload.
    pub(crate) fn upload_glyph(&mut self, glyph: Arc<Glyph>, padding: u16, tex_handle: &mut TextureHandle) {
        self.queue(tex_handle, QueuedPixels::Glyph { glyph, padding });
    }

    fn queue(&mut self, tex_handle: &mut TextureHandle, pixels: QueuedPixels) {
        self.images_to_upload.entry(tex_handle.id).or_insert((tex_handle.rectangle, pixels));
        tex_handle.needs_upload = false;
    }

    /// Queues new pixels for the bin with id `id`, replacing any still
    /// queued for it: a same-size image update.
    pub(crate) fn queue_upload(&mut self, id: DynamicTextureBinId, rect: RectU16, data: Vec<u8>) {
        self.images_to_upload.insert(id, (rect, QueuedPixels::Bytes(data)));
    }

    /// The pixels queued for the frame's next upload, by handle id.
    #[cfg(test)]
    pub(crate) fn queued_uploads(&self) -> &HashMap<DynamicTextureBinId, (RectU16, QueuedPixels)> {
        &self.images_to_upload
    }

    /// Makes the GPU texture on first use and uploads the queued images.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.texture.is_none() {
            let texture = context.add_texture_with(|texture| {
                texture.set_size(texture_size(self.size));
                texture.set_format(texture_format(self.pixel_format, TextureChannelDataType::UnsignedByte));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..SamplerState::default()
                });
            });
            if let Err(error) = context.create_texture(gpu, &texture) {
                context.fail(format!("Making an atlas texture failed: {error}"));
            }
            self.texture = Some(texture);
        }
        let Some(texture) = self.texture.as_ref() else {
            return;
        };
        if self.images_to_upload.is_empty() {
            return;
        }
        // The padded pixels of the glyph being uploaded, reused across bins.
        let mut padded = Vec::new();
        for (rect, pixels) in self.images_to_upload.values() {
            let data = match pixels {
                QueuedPixels::Bytes(data) => data,
                QueuedPixels::Glyph { glyph, padding } => {
                    pad_glyph(&glyph.bitmap, *rect, *padding, &mut padded);
                    &padded
                }
            };
            let size = Size::new(u32::from(rect.w), u32::from(rect.h));
            if let Err(error) = context.upload_texture_region(gpu, texture, data, texture_size(size), rect.x, rect.y) {
                context.fail(format!("Uploading to an atlas texture failed: {error}"));
            }
        }
        self.images_to_upload.clear();
    }

    /// Releases `tex_handle`'s bin; true when that was its last reference.
    pub(crate) fn remove_texture(&mut self, tex_handle: &TextureHandle) -> bool {
        let Some(bin) = self.bins.get_mut(&tex_handle.id) else {
            return false;
        };
        bin.refcount -= 1;
        if bin.refcount > 0 {
            return false;
        }
        let alloc_id = bin.alloc_id;
        self.bins.remove(&tex_handle.id);
        self.allocator.deallocate(alloc_id);
        self.images_to_upload.remove(&tex_handle.id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_ids_are_nonzero_and_do_not_wrap() {
        assert!(DynamicTextureBinId::new(0).is_none());
        assert_eq!(DynamicTextureBinId::new(u32::MAX).unwrap().next(), None);
    }

    #[test]
    fn reserve_size_allocates_fresh_ids_and_reuses_explicit_ids() {
        let mut texture = DynamicTexture::new(Size::new(64, 64), TexturePixelType::Alpha);
        let size = Size::new(8, 8);
        let first = texture.reserve_size(size, None).expect("first bin");
        let second = texture.reserve_size(size, None).expect("second bin");
        let reused = texture.reserve_size(size, Some(first.id())).expect("existing bin");

        assert_eq!(first.id(), DynamicTextureBinId::new(1).unwrap());
        assert_eq!(second.id(), DynamicTextureBinId::new(2).unwrap());
        assert_eq!(reused.id(), first.id());
        assert!(!reused.is_upload_needed());
        assert_eq!(reused.rectangle(), first.rectangle());
    }
}
