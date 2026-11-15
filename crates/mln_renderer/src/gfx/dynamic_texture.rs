// A page of the dynamic texture atlas (dynamic_texture_atlas.rs), which owns
// it on the render thread. The GPU texture is made in the first `upload`, as
// the C++ WebGPU texture is (`deferredCreation`), and shared with the tiles'
// leases through a `OnceCell`, so a tile's render data can bind it without
// reaching the atlas.

use std::cell::OnceCell;
use std::num::NonZeroU32;
use std::rc::Rc;

use etagere::{AllocId, AtlasAllocator, size2};
use mln_gfx::texture2d::{SamplerState, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_style_types::image::RectU16;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;

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
    pub(crate) fn get_id(&self) -> DynamicTextureBinId {
        self.id
    }

    #[must_use]
    pub(crate) fn get_rectangle(&self) -> &RectU16 {
        &self.rectangle
    }

    #[must_use]
    pub(crate) fn is_upload_needed(&self) -> bool {
        self.needs_upload
    }
}

/// `operator==`: handles compare by id.
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

/// A page's GPU texture, once the page's first `upload` made it. Shared with
/// the leases on the page's bins.
pub(crate) type PageTexture = Rc<OnceCell<Texture2DPtr>>;

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
    images_to_upload: HashMap<DynamicTextureBinId, (RectU16, Vec<u8>)>,
    texture: PageTexture,
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

impl DynamicTexture {
    pub(crate) fn new(size: Size, pixel_type: TexturePixelType) -> Self {
        DynamicTexture {
            size,
            pixel_format: pixel_type,
            allocator: AtlasAllocator::new(size2(size.width as i32, size.height as i32)),
            bins: HashMap::default(),
            max_id: None,
            images_to_upload: HashMap::default(),
            texture: PageTexture::default(),
        }
    }

    /// The page's GPU texture, shared with the leases on its bins.
    pub(crate) fn texture(&self) -> &PageTexture {
        &self.texture
    }

    pub(crate) fn get_pixel_format(&self) -> TexturePixelType {
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
        pixel_data: &[u8],
        image_size: Size,
        id: Option<DynamicTextureBinId>,
    ) -> Option<TextureHandle> {
        let mut tex_handle = self.reserve_size(image_size, id)?;
        if tex_handle.is_upload_needed() {
            self.upload_image(pixel_data, &mut tex_handle);
        }
        Some(tex_handle)
    }

    /// Queues the pixels of the handle's rectangle for the frame's next
    /// upload.
    pub(crate) fn upload_image(&mut self, pixel_data: &[u8], tex_handle: &mut TextureHandle) {
        let rect = tex_handle.rectangle;
        let byte_size = rect.w as usize * rect.h as usize * pixel_stride(self.pixel_format);
        let image_data = pixel_data[..byte_size].to_vec();
        self.images_to_upload.entry(tex_handle.id).or_insert((rect, image_data));
        tex_handle.needs_upload = false;
    }

    /// Queues new pixels for the bin with id `id`, replacing any still
    /// queued for it: a same-size image update.
    pub(crate) fn queue_upload(&mut self, id: DynamicTextureBinId, rect: RectU16, data: Vec<u8>) {
        self.images_to_upload.insert(id, (rect, data));
    }

    /// The pixels queued for the frame's next upload, by handle id.
    #[cfg(test)]
    pub(crate) fn queued_uploads(&self) -> &HashMap<DynamicTextureBinId, (RectU16, Vec<u8>)> {
        &self.images_to_upload
    }

    /// Makes the GPU texture on first use and uploads the queued images.
    pub(crate) fn upload(&mut self, context: &Context) {
        let texture = self.texture.get_or_init(|| {
            let texture = context.create_texture_2d();
            {
                let mut t = texture.borrow_mut();
                t.set_size(self.size);
                t.set_format(texture_format(self.pixel_format, TextureChannelDataType::UnsignedByte));
                t.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..SamplerState::default()
                });
                if let Err(error) = t.create(context) {
                    Log::error(Event::Render, format!("Making an atlas texture failed: {error}"));
                }
            }
            texture
        });
        if self.images_to_upload.is_empty() {
            return;
        }
        let mut t = texture.borrow_mut();
        for (rect, data) in self.images_to_upload.values() {
            let size = Size::new(u32::from(rect.w), u32::from(rect.h));
            if let Err(error) = t.upload_sub_region(context, data, size, rect.x, rect.y) {
                Log::error(Event::Render, format!("Uploading to an atlas texture failed: {error}"));
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
        let reused = texture.reserve_size(size, Some(first.get_id())).expect("existing bin");

        assert_eq!(first.get_id(), DynamicTextureBinId::new(1).unwrap());
        assert_eq!(second.get_id(), DynamicTextureBinId::new(2).unwrap());
        assert_eq!(reused.get_id(), first.get_id());
        assert!(!reused.is_upload_needed());
        assert_eq!(reused.get_rectangle(), first.get_rectangle());
    }
}
