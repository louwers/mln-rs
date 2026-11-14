// C++ shares a `DynamicTexture` between the worker threads (which reserve
// bins and queue images) and the render thread (which uploads them). The
// atlas allocator and the queued images are behind a mutex here as there;
// the GPU texture is render-thread state (`Texture2DPtr` is `Rc`), so it is
// a `LocalObject` (a Send handle to a render-thread value) created in the
// first `upload_deferred_images`, as the C++ WebGPU texture is
// (`deferredCreation`). Dropping the DynamicTexture on the render thread
// frees the GPU texture (and with the last one, the map's device).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use etagere::{AllocId, AtlasAllocator, size2};
use mln_actor::LocalObject;
use mln_gfx::Context;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_style_types::image::RectU16;
use mln_util::size::Size;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// A bin of a dynamic texture.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct TextureHandle {
    id: i32,
    rectangle: RectU16,
    needs_upload: bool,
}

impl TextureHandle {
    #[must_use]
    pub(crate) fn get_id(&self) -> i32 {
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

struct Inner {
    allocator: AtlasAllocator,
    /// The allocated bins, by handle id.
    bins: HashMap<i32, Bin>,
    /// The highest handle id given out or asked for; a new bin without an id
    /// takes the next one.
    max_id: i32,
    /// The images queued for the next upload pass, keyed by handle id.
    images_to_upload: HashMap<i32, (RectU16, Vec<u8>)>,
}

pub(crate) type DynamicTexturePtr = Arc<DynamicTexture>;

static NEXT_DYNAMIC_TEXTURE_ID: AtomicUsize = AtomicUsize::new(1);

/// The render-thread GPU texture of a dynamic texture.
type GpuTexture = LocalObject<RefCell<Option<Texture2DPtr>>>;

/// A texture atlas of images or glyphs. Workers reserve bins and queue
/// images; the render thread uploads them.
pub(crate) struct DynamicTexture {
    id: usize,
    size: Size,
    pixel_format: TexturePixelType,
    inner: Mutex<Inner>,
    gpu_texture: Mutex<Option<GpuTexture>>,
}

impl std::fmt::Debug for DynamicTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicTexture").field("id", &self.id).field("size", &self.size).finish_non_exhaustive()
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
            id: NEXT_DYNAMIC_TEXTURE_ID.fetch_add(1, Ordering::Relaxed),
            size,
            pixel_format: pixel_type,
            inner: Mutex::new(Inner {
                allocator: AtlasAllocator::new(size2(size.width as i32, size.height as i32)),
                bins: HashMap::new(),
                max_id: 0,
                images_to_upload: HashMap::new(),
            }),
            gpu_texture: Mutex::new(None),
        }
    }

    pub(crate) fn id(&self) -> usize {
        self.id
    }

    pub(crate) fn get_size(&self) -> Size {
        self.size
    }

    /// The GPU texture, once `upload_deferred_images` has created it. Render
    /// thread only.
    pub(crate) fn get_texture(&self) -> Option<Texture2DPtr> {
        let gpu_texture = self.gpu_texture.lock();
        gpu_texture.as_ref().and_then(LocalObject::get).and_then(|t| t.borrow().clone())
    }

    pub(crate) fn get_pixel_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inner.lock().bins.is_empty()
    }

    /// Reserves a bin of `size`, or another reference to the bin with id
    /// `unique_id` if it has one. A `unique_id` of -1 asks for a new bin with
    /// a fresh id. The handle needs an upload when its bin is new.
    pub(crate) fn reserve_size(&self, size: Size, unique_id: i32) -> Option<TextureHandle> {
        let mut inner = self.inner.lock();
        if let Some(bin) = inner.bins.get_mut(&unique_id) {
            bin.refcount += 1;
            return Some(TextureHandle { id: unique_id, rectangle: bin.rectangle, needs_upload: false });
        }
        let width = u16::try_from(size.width).ok()?;
        let height = u16::try_from(size.height).ok()?;
        let allocation = inner.allocator.allocate(size2(i32::from(width), i32::from(height)))?;
        let id = if unique_id == -1 { inner.max_id + 1 } else { unique_id };
        inner.max_id = inner.max_id.max(id);
        // The allocation can be taller than asked for; the bin is the size
        // asked for, at its corner.
        let min = allocation.rectangle.min;
        let rectangle = RectU16 { x: min.x as u16, y: min.y as u16, w: width, h: height };
        inner.bins.insert(id, Bin { alloc_id: allocation.id, rectangle, refcount: 1 });
        Some(TextureHandle { id, rectangle, needs_upload: true })
    }

    pub(crate) fn add_image(&self, pixel_data: &[u8], image_size: Size, unique_id: i32) -> Option<TextureHandle> {
        let mut tex_handle = self.reserve_size(image_size, unique_id)?;
        if tex_handle.is_upload_needed() {
            self.upload_image(pixel_data, &mut tex_handle);
        }
        Some(tex_handle)
    }

    /// Queues the pixels of the handle's rectangle for the next upload pass.
    pub(crate) fn upload_image(&self, pixel_data: &[u8], tex_handle: &mut TextureHandle) {
        let mut inner = self.inner.lock();
        let rect = tex_handle.rectangle;
        let byte_size = rect.w as usize * rect.h as usize * pixel_stride(self.pixel_format);
        let image_data = pixel_data[..byte_size].to_vec();
        inner.images_to_upload.entry(tex_handle.id).or_insert((rect, image_data));
        tex_handle.needs_upload = false;
    }

    /// Creates the texture on first use and uploads the queued images. Runs
    /// on the render thread.
    pub(crate) fn upload_deferred_images(&self, context: &dyn Context) {
        let mut inner = self.inner.lock();
        let texture = if let Some(texture) = self.get_texture() {
            texture
        } else {
            let texture = context.create_texture_2d();
            {
                let mut t = texture.borrow_mut();
                t.set_size(self.size);
                t.set_format(self.pixel_format, TextureChannelDataType::UnsignedByte);
                t.set_sampler_configuration(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..SamplerState::default()
                });
                t.create();
            }
            *self.gpu_texture.lock() = Some(LocalObject::new(RefCell::new(Some(std::rc::Rc::clone(&texture)))));
            texture
        };
        let mut t = texture.borrow_mut();
        for (rect, data) in inner.images_to_upload.values() {
            t.upload_sub_region(Some(data), Size::new(u32::from(rect.w), u32::from(rect.h)), rect.x, rect.y);
        }
        inner.images_to_upload.clear();
    }

    /// Port-only: takes the queued images (handle id, rectangle, pixels),
    /// in handle order, for a mirror of this texture in another wasm
    /// instance (`tile/worker_codec`), which uploads them instead.
    pub(crate) fn take_uploads(&self) -> Vec<(i32, RectU16, Vec<u8>)> {
        let mut inner = self.inner.lock();
        let mut uploads: Vec<_> = inner.images_to_upload.drain().map(|(id, (rect, data))| (id, rect, data)).collect();
        uploads.sort_by_key(|(id, _, _)| *id);
        uploads
    }

    /// Queues new pixels for a bin: a same-size image update, or a bin that
    /// the texture this one mirrors reserved (`take_uploads`). They replace
    /// whatever is still queued for the bin or for the space it takes: a
    /// mirrored texture reserves a space again only after freeing it, so
    /// pixels queued there before belong to a bin that is gone, and would
    /// otherwise be uploaded over these in whatever order the queue is
    /// drained.
    pub(crate) fn queue_upload(&self, id: i32, rect: RectU16, data: Vec<u8>) {
        let span = |start: u16, length: u16| u32::from(start)..u32::from(start) + u32::from(length);
        let intersect = |a: std::ops::Range<u32>, b: std::ops::Range<u32>| a.start < b.end && b.start < a.end;
        let overlaps = |other: &RectU16| {
            intersect(span(other.x, other.w), span(rect.x, rect.w))
                && intersect(span(other.y, other.h), span(rect.y, rect.h))
        };
        let mut inner = self.inner.lock();
        inner.images_to_upload.retain(|_, (queued, _)| !overlaps(queued));
        inner.images_to_upload.insert(id, (rect, data));
    }

    /// Releases `tex_handle`'s bin; true when that was its last reference.
    pub(crate) fn remove_texture(&self, tex_handle: &TextureHandle) -> bool {
        let mut inner = self.inner.lock();
        let Some(bin) = inner.bins.get_mut(&tex_handle.id) else {
            return false;
        };
        bin.refcount -= 1;
        if bin.refcount > 0 {
            return false;
        }
        let alloc_id = bin.alloc_id;
        inner.bins.remove(&tex_handle.id);
        inner.allocator.deallocate(alloc_id);
        inner.images_to_upload.remove(&tex_handle.id);
        true
    }

    /// Drops the render-thread GPU texture (render thread; a no-op
    /// elsewhere).
    pub(crate) fn release_gpu_texture(&self) {
        let gpu_texture = self.gpu_texture.lock().take();
        if let Some(gpu_texture) = gpu_texture
            && let Some(cell) = gpu_texture.get()
        {
            cell.borrow_mut().take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixels queued for a bin of a mirrored texture replace those still
    /// queued for a freed bin in the same space.
    #[test]
    fn queued_uploads_replace_the_space_they_take() {
        let texture = DynamicTexture::new(Size::new(64, 64), TexturePixelType::Alpha);
        let rect = |x, y, w, h| RectU16 { x, y, w, h };
        texture.queue_upload(1, rect(0, 0, 8, 8), vec![1; 64]);
        texture.queue_upload(2, rect(8, 0, 8, 8), vec![2; 64]);
        texture.queue_upload(3, rect(4, 4, 2, 2), vec![3; 4]);
        let uploads: Vec<(i32, RectU16)> = texture.take_uploads().into_iter().map(|(id, rect, _)| (id, rect)).collect();
        assert_eq!(uploads, vec![(2, rect(8, 0, 8, 8)), (3, rect(4, 4, 2, 2))]);
    }
}
