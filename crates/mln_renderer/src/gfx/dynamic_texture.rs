// Ports include/mln/gfx/dynamic_texture.hpp, src/mln/gfx/dynamic_texture.cpp
// and src/mln/webgpu/dynamic_texture.cpp (the WebGPU backend's deferred
// creation and uploads).
//
// C++ shares a `DynamicTexture` between the worker threads (which reserve
// bins and queue images) and the render thread (which uploads them). The
// shelf pack and the queued images are behind a mutex here as there; the GPU
// texture is render-thread state (`Texture2DPtr` is `Rc`), so it is a
// `LocalObject` (a Send handle to a render-thread value) created in the
// first `upload_deferred_images`, as the C++ WebGPU texture is
// (`deferredCreation`). Dropping the DynamicTexture on the render thread
// frees the GPU texture (and with the last one, the map's device).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mln_actor::LocalObject;
use mln_gfx::Context;
use mln_gfx::texture2d::{SamplerState, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_style_types::image::RectU16;
use mln_util::size::Size;
use mln_util::sync::MutexExt;
use shelf_pack::{ShelfPack, ShelfPackOptions};

/// `TextureHandle`: a bin of a dynamic texture.
#[derive(Clone, Copy, Debug)]
pub struct TextureHandle {
    id: i32,
    rectangle: RectU16,
    needs_upload: bool,
}

impl TextureHandle {
    /// `TextureHandle(const mapbox::Bin&)`.
    fn from_bin(bin: &shelf_pack::Bin) -> Self {
        TextureHandle {
            id: bin.id,
            rectangle: RectU16 { x: bin.x as u16, y: bin.y as u16, w: bin.w as u16, h: bin.h as u16 },
            needs_upload: bin.refcount() == 1,
        }
    }

    pub fn get_id(&self) -> i32 {
        self.id
    }

    pub fn get_rectangle(&self) -> &RectU16 {
        &self.rectangle
    }

    pub fn is_upload_needed(&self) -> bool {
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

struct Inner {
    shelf_pack: ShelfPack,
    num_textures: i32,
    /// `imagesToUpload` (webgpu::DynamicTexture), keyed by handle id.
    images_to_upload: HashMap<i32, (RectU16, Vec<u8>)>,
}

pub type DynamicTexturePtr = Arc<DynamicTexture>;

static NEXT_DYNAMIC_TEXTURE_ID: AtomicUsize = AtomicUsize::new(1);

/// The render-thread GPU texture of a dynamic texture.
type GpuTexture = LocalObject<RefCell<Option<Texture2DPtr>>>;

/// `gfx::DynamicTexture` (with the `webgpu::DynamicTexture` overrides).
pub struct DynamicTexture {
    id: usize,
    size: Size,
    pixel_format: TexturePixelType,
    inner: Mutex<Inner>,
    gpu_texture: Mutex<Option<GpuTexture>>,
}

impl std::fmt::Debug for DynamicTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicTexture").field("id", &self.id).field("size", &self.size).finish()
    }
}

fn pixel_stride(pixel_type: TexturePixelType) -> usize {
    match pixel_type {
        TexturePixelType::Alpha | TexturePixelType::Luminance => 1,
        TexturePixelType::RGBA | TexturePixelType::Stencil | TexturePixelType::Depth => 4,
    }
}

impl DynamicTexture {
    /// `Context::createDynamicTexture(size, pixelType)`.
    pub fn new(size: Size, pixel_type: TexturePixelType) -> Self {
        DynamicTexture {
            id: NEXT_DYNAMIC_TEXTURE_ID.fetch_add(1, Ordering::Relaxed),
            size,
            pixel_format: pixel_type,
            inner: Mutex::new(Inner {
                shelf_pack: ShelfPack::new(
                    size.width as i32,
                    size.height as i32,
                    ShelfPackOptions { auto_resize: false },
                ),
                num_textures: 0,
                images_to_upload: HashMap::new(),
            }),
            gpu_texture: Mutex::new(None),
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn get_size(&self) -> Size {
        self.size
    }

    /// `getTexture()`: the GPU texture, once `upload_deferred_images` created
    /// it (render thread).
    pub fn get_texture(&self) -> Option<Texture2DPtr> {
        let gpu_texture = self.gpu_texture.lock_unpoisoned();
        gpu_texture.as_ref().and_then(|t| t.get()).and_then(|t| t.borrow().clone())
    }

    pub fn get_pixel_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock_unpoisoned().num_textures == 0
    }

    /// `reserveSize(size, uniqueId)`.
    pub fn reserve_size(&self, size: Size, unique_id: i32) -> Option<TextureHandle> {
        let mut inner = self.inner.lock_unpoisoned();
        let bin_ref = inner.shelf_pack.pack_one(unique_id, size.width as i32, size.height as i32)?;
        let bin = *inner.shelf_pack.bin(bin_ref);
        if bin.refcount() == 1 {
            inner.num_textures += 1;
        }
        Some(TextureHandle::from_bin(&bin))
    }

    /// `addImage(pixelData, imageSize, uniqueId)`.
    pub fn add_image(&self, pixel_data: &[u8], image_size: Size, unique_id: i32) -> Option<TextureHandle> {
        let mut tex_handle = self.reserve_size(image_size, unique_id)?;
        if tex_handle.is_upload_needed() {
            self.upload_image(pixel_data, &mut tex_handle);
        }
        Some(tex_handle)
    }

    /// `uploadImage(pixelData, texHandle)` (webgpu: queue the pixels of the
    /// handle's rectangle for the next upload pass).
    pub fn upload_image(&self, pixel_data: &[u8], tex_handle: &mut TextureHandle) {
        let mut inner = self.inner.lock_unpoisoned();
        let rect = tex_handle.rectangle;
        let byte_size = rect.w as usize * rect.h as usize * pixel_stride(self.pixel_format);
        let image_data = pixel_data[..byte_size].to_vec();
        inner.images_to_upload.entry(tex_handle.id).or_insert((rect, image_data));
        tex_handle.needs_upload = false;
    }

    /// `uploadDeferredImages(UploadPass&)`: creates the texture on first use
    /// and uploads the queued images (render thread).
    pub fn upload_deferred_images(&self, context: &dyn Context) {
        let mut inner = self.inner.lock_unpoisoned();
        let texture = match self.get_texture() {
            Some(texture) => texture,
            None => {
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
                *self.gpu_texture.lock_unpoisoned() = Some(LocalObject::new(RefCell::new(Some(texture.clone()))));
                texture
            }
        };
        let mut t = texture.borrow_mut();
        for (rect, data) in inner.images_to_upload.values() {
            t.upload_sub_region(Some(data), Size::new(rect.w as u32, rect.h as u32), rect.x, rect.y);
        }
        inner.images_to_upload.clear();
    }

    /// `removeTexture(texHandle)`: true when the bin's last reference went.
    pub fn remove_texture(&self, tex_handle: &TextureHandle) -> bool {
        let mut inner = self.inner.lock_unpoisoned();
        let Some(bin) = inner.shelf_pack.get_bin(tex_handle.id) else {
            return false;
        };
        let refcount = inner.shelf_pack.unref(bin);
        if refcount == 0 {
            inner.num_textures -= 1;
            inner.images_to_upload.remove(&tex_handle.id);
            return true;
        }
        false
    }

    /// Drops the render-thread GPU texture (render thread; a no-op
    /// elsewhere).
    pub(crate) fn release_gpu_texture(&self) {
        let gpu_texture = self.gpu_texture.lock_unpoisoned().take();
        if let Some(gpu_texture) = gpu_texture {
            if let Some(cell) = gpu_texture.get() {
                cell.borrow_mut().take();
            }
        }
    }
}
