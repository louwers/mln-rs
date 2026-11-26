//! The renderer's textures. Each is a [`Texture2D`] in the context's store,
//! which its owner and the draws that bind it hold as a
//! [`SharedTexture`](crate::gfx::store::SharedTexture).

use std::rc::Rc;

use mln_gfx::Size;
use mln_gfx::gpu::{Gpu, GpuError, SamplerId, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::texture2d::SamplerState;
use mln_style::bitmap::PremultipliedImage;

use crate::gfx::resources::{Device, SharedGpuTexture};
use crate::gfx::texture_size;

/// A texture of the renderer: its size, format and sampler state, the image
/// it waits to upload, and once made, a texture of the backend's
/// [`Gpu`](mln_gfx::gpu::Gpu), which the texture lets go of when it
/// replaces it or drops, and which bindings made of it hold until they go. The context changes it, and lends it to set up as a
/// [`TextureMut`].
pub(crate) struct Texture2D {
    texture: Option<SharedGpuTexture>,
    size: Size,
    format: TextureFormat,
    render_target: bool,
    /// Whether it is made with a full chain of mip levels.
    mipmapped: bool,
    sampler: SamplerState,
    /// The context's sampler of `sampler`, once a draw bound it.
    sampler_id: Option<SamplerId>,
    /// The image the next upload writes.
    image: Option<Rc<PremultipliedImage>>,
}

impl Texture2D {
    /// An RGBA texture without a size.
    pub(crate) fn new() -> Texture2D {
        Texture2D {
            texture: None,
            size: Size::new(0, 0),
            format: TextureFormat::Rgba8Unorm,
            render_target: false,
            mipmapped: false,
            sampler: SamplerState::default(),
            sampler_id: None,
            image: None,
        }
    }

    #[must_use]
    pub(crate) fn size(&self) -> Size {
        self.size
    }

    /// Sets the size the texture is made with. A texture made with another
    /// size goes.
    fn set_size(&mut self, size: Size) {
        if size != self.size {
            self.release();
            self.size = size;
        }
    }

    #[must_use]
    pub(crate) fn format(&self) -> TextureFormat {
        self.format
    }

    /// Sets the format the texture is made with. A texture made with
    /// another format goes.
    fn set_format(&mut self, format: TextureFormat) {
        if format != self.format {
            self.release();
            self.format = format;
        }
    }

    /// How shaders sample the texture.
    #[must_use]
    pub(crate) fn sampler(&self) -> SamplerState {
        self.sampler
    }

    /// The context's sampler of the texture's sampler state, once a draw
    /// bound it.
    pub(crate) fn sampler_id(&self) -> Option<SamplerId> {
        self.sampler_id
    }

    pub(crate) fn set_sampler_id(&mut self, sampler: SamplerId) {
        self.sampler_id = Some(sampler);
    }

    /// Whether the texture has an image to upload.
    pub(crate) fn has_image(&self) -> bool {
        self.image.is_some()
    }

    /// The backend's texture, once made, which bindings made of it hold.
    pub(crate) fn gpu_texture(&self) -> Option<&SharedGpuTexture> {
        self.texture.as_ref()
    }

    /// Makes the backend's texture of the size and format set through `gpu`
    /// and `device`, unless it is made. Fails for an empty size.
    pub(crate) fn create(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<TextureId, GpuError> {
        if let Some(texture) = &self.texture {
            return Ok(texture.id());
        }
        let desc = TextureDesc {
            label: if self.render_target { "render target" } else { "texture" },
            size: self.size,
            format: self.format,
            usage: TextureUsage { sampled: true, render_target: self.render_target, copy_src: self.render_target },
            mipmapped: self.mipmapped,
        };
        let texture = device.texture(gpu, &desc, Some(self.bytes()))?;
        let id = texture.id();
        self.texture = Some(Rc::new(texture));
        Ok(id)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture, first making it of that size. Empty `data`
    /// leaves the texels unspecified, and an empty size makes no texture.
    pub(crate) fn upload_data(
        &mut self,
        gpu: &mut dyn Gpu,
        device: &mut Device,
        data: &[u8],
        size: Size,
    ) -> Result<(), GpuError> {
        self.set_size(size);
        if size.is_empty() {
            return Ok(());
        }
        let texture = self.create(gpu, device)?;
        if data.is_empty() {
            return Ok(());
        }
        write(gpu, &mut device.stats, texture, data, TextureRegion { x: 0, y: 0, size })
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture, first making it.
    pub(crate) fn upload_sub_region(
        &mut self,
        gpu: &mut dyn Gpu,
        device: &mut Device,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let texture = self.create(gpu, device)?;
        let region = TextureRegion { x: u32::from(x), y: u32::from(y), size };
        write(gpu, &mut device.stats, texture, data, region)
    }

    /// Writes the image [`TextureMut::set_image`] set, if any.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<(), GpuError> {
        match self.image.take() {
            Some(image) if image.valid() => self.upload_data(gpu, device, &image.data, texture_size(image.size)),
            _ => Ok(()),
        }
    }

    /// The texture a draw binds, made through `gpu` and holding its image.
    pub(crate) fn bind(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<TextureId, GpuError> {
        self.upload(gpu, device)?;
        self.create(gpu, device)
    }

    /// The bytes the texture takes, its mip levels' included.
    pub(crate) fn bytes(&self) -> usize {
        let texel = self.format.bytes_per_texel().unwrap_or(4) as usize;
        let bytes = self.size.width as usize * self.size.height as usize * texel;
        // The levels below the first add up to a third of it.
        if self.mipmapped { bytes + bytes / 3 } else { bytes }
    }

    /// Lets go of the backend's texture, which goes once the bindings made
    /// of it do.
    fn release(&mut self) {
        self.texture = None;
    }
}

/// A texture of the context's store, to set up: changes that replace its
/// backend's texture let go of that.
pub(crate) struct TextureMut<'a> {
    texture: &'a mut Texture2D,
}

impl<'a> TextureMut<'a> {
    pub(crate) fn new(texture: &'a mut Texture2D) -> TextureMut<'a> {
        TextureMut { texture }
    }

    /// Sets the size the texture is made with. A texture made with another
    /// size goes.
    pub(crate) fn set_size(&mut self, size: Size) {
        self.texture.set_size(size);
    }

    /// Sets the format the texture is made with. A texture made with
    /// another format goes.
    pub(crate) fn set_format(&mut self, format: TextureFormat) {
        self.texture.set_format(format);
    }

    /// Makes the texture one that passes draw into, as well as one that
    /// shaders sample.
    pub(crate) fn set_render_target(&mut self) {
        self.texture.render_target = true;
    }

    /// Makes the texture with a full chain of mip levels, or with one. A
    /// texture made otherwise goes.
    pub(crate) fn set_mipmapped(&mut self, mipmapped: bool) {
        if mipmapped != self.texture.mipmapped {
            self.texture.release();
            self.texture.mipmapped = mipmapped;
        }
    }

    /// Sets how shaders sample the texture, with an anisotropy of at least 1.
    pub(crate) fn set_sampler(&mut self, sampler: SamplerState) {
        let texture = &mut *self.texture;
        let sampler = SamplerState { max_anisotropy: sampler.max_anisotropy.max(1), ..sampler };
        if sampler != texture.sampler {
            texture.sampler = sampler;
            texture.sampler_id = None;
        }
    }

    /// Sets the RGBA image the next upload writes, and the texture's size to
    /// the image's.
    pub(crate) fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        if let Some(image) = &image {
            self.set_size(texture_size(image.size));
            self.set_format(TextureFormat::Rgba8Unorm);
        }
        self.texture.image = image;
    }
}

/// Writes `data` to `region` of `texture` through `gpu`, and counts the
/// write in `stats`.
fn write(
    gpu: &mut dyn Gpu,
    stats: &mut RenderingStats,
    texture: TextureId,
    data: &[u8],
    region: TextureRegion,
) -> Result<(), GpuError> {
    gpu.write_texture(texture, region, data)?;
    stats.num_texture_updates += 1;
    stats.texture_update_bytes += data.len();
    Ok(())
}
