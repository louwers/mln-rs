//! The renderer's textures. Each is a [`Texture2D`] in the context's store,
//! which its owner and the draws that bind it hold as a
//! [`SharedTexture`](crate::gfx::store::SharedTexture).

use std::rc::Rc;

use mln_gfx::gpu::{Gpu, GpuError, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
use mln_gfx::rendering_stats::RenderingStats;
use mln_util::image::{AlphaImage, PremultipliedImage};
use mln_util::size::Size;

use crate::gfx::resources::{Device, GpuTexture, SharedGpuTexture};

/// A texture of the renderer: its size and format, the image
/// it waits to upload, and once made, a texture of the backend's
/// [`Gpu`](mln_gfx::gpu::Gpu), which the texture lets go of when it
/// replaces it or drops, and which bindings made of it hold until they go. The context changes it, and lends it to set up as a
/// `TextureMut`.
pub(crate) struct Texture2D {
    texture: Option<SharedGpuTexture>,
    size: Size,
    format: TextureFormat,
    render_target: bool,
    /// Whether the backend's texture holds what a pass drew into it.
    drawn: bool,
    /// The depth that passes drawing into a render target draw with, of its
    /// size, once one did.
    depth: Option<GpuTexture>,
    /// The image the next upload writes.
    image: Option<Pending>,
}

/// An image a texture waits to upload, of its format.
enum Pending {
    Rgba(Rc<PremultipliedImage>),
    Alpha(Rc<AlphaImage>),
    Texels(Rc<[u8]>, Size),
}

impl Pending {
    /// The image's texels and size, `None` for an image without texels.
    fn texels(&self) -> Option<(&[u8], Size)> {
        let (valid, data, size) = match self {
            Pending::Rgba(image) => (image.valid(), &image.data[..], image.size),
            Pending::Alpha(image) => (image.valid(), &image.data[..], image.size),
            Pending::Texels(data, size) => (!data.is_empty() && !size.is_empty(), &data[..], *size),
        };
        valid.then_some((data, size))
    }
}

impl Texture2D {
    /// Whether passes draw into the texture.
    pub(crate) fn is_render_target(&self) -> bool {
        self.render_target
    }

    /// Whether the backend's texture holds what a pass drew into it, since
    /// the pass ([`set_drawn`](Self::set_drawn)).
    pub(crate) fn is_drawn(&self) -> bool {
        self.drawn && self.texture.is_some()
    }

    /// Says whether a pass drew into the backend's texture without a
    /// failure: what it drew stays until the texture goes.
    pub(crate) fn set_drawn(&mut self, drawn: bool) {
        self.drawn = drawn;
    }

    /// An RGBA texture without a size.
    pub(crate) fn new() -> Texture2D {
        Texture2D {
            texture: None,
            size: Size::new(0, 0),
            format: TextureFormat::Rgba8Unorm,
            render_target: false,
            drawn: false,
            depth: None,
            image: None,
        }
    }

    /// The texture's size in texels, zero before it has an image.
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

    /// Writes the image [`TextureMut::set_image`],
    /// [`TextureMut::set_alpha_image`] or [`TextureMut::set_texels`] set, if
    /// any.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<(), GpuError> {
        let Some(image) = self.image.take() else { return Ok(()) };
        match image.texels() {
            Some((data, size)) => self.upload_data(gpu, device, data, size),
            None => Ok(()),
        }
    }

    /// The texture a draw binds, made through `gpu` and holding its image.
    pub(crate) fn bind(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<TextureId, GpuError> {
        self.upload(gpu, device)?;
        self.create(gpu, device)
    }

    /// The bytes the texture takes.
    fn bytes(&self) -> usize {
        let texel = self.format.bytes_per_texel().unwrap_or(4) as usize;
        self.size.width as usize * self.size.height as usize * texel
    }

    /// The depth texture of `format` that a pass drawing into the texture,
    /// a render target, draws with, made through `gpu` and `device` unless
    /// it is.
    pub(crate) fn depth(
        &mut self,
        gpu: &mut dyn Gpu,
        device: &mut Device,
        format: TextureFormat,
    ) -> Result<TextureId, GpuError> {
        if let Some(depth) = &self.depth {
            return Ok(depth.id());
        }
        let depth = device.attachment(gpu, "render target depth", self.size, format)?;
        Ok(self.depth.insert(depth).id())
    }

    /// Lets go of the backend's texture, which goes once the bindings made
    /// of it do, and of its depth.
    fn release(&mut self) {
        self.texture = None;
        self.drawn = false;
        self.depth = None;
    }
}

/// A texture of the context's store, to set up: changes that replace its
/// backend's texture let go of that.
pub struct TextureMut<'a> {
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
    pub fn set_format(&mut self, format: TextureFormat) {
        self.texture.set_format(format);
    }

    /// Makes the texture one that passes draw into, as well as one that
    /// shaders sample.
    pub(crate) fn set_render_target(&mut self) {
        self.texture.render_target = true;
    }

    /// Sets the RGBA image the next upload writes, and the texture's size to
    /// the image's.
    pub fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        if let Some(image) = &image {
            self.set_size(image.size);
            self.set_format(TextureFormat::Rgba8Unorm);
        }
        self.texture.image = image.map(Pending::Rgba);
    }

    /// Sets the one-channel image the next upload writes, which shaders
    /// read as alpha, and the texture's size to the image's.
    pub fn set_alpha_image(&mut self, image: Rc<AlphaImage>) {
        self.set_size(image.size);
        self.set_format(TextureFormat::A8Unorm);
        self.texture.image = Some(Pending::Alpha(image));
    }

    /// Sets the texels the next upload writes, `data`, `size` texels of
    /// `format` in rows from the top, and the texture's size and format to
    /// theirs.
    pub fn set_texels(&mut self, format: TextureFormat, size: Size, data: Rc<[u8]>) {
        debug_assert_eq!(
            format.bytes_per_texel().map(|bytes| bytes as usize * size.width as usize * size.height as usize),
            Some(data.len()),
            "texels of another size than their format's"
        );
        self.set_size(size);
        self.set_format(format);
        self.texture.image = Some(Pending::Texels(data, size));
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
