use std::cell::RefCell;
use std::rc::Rc;

use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::context::Context;
use crate::gpu::{GpuError, SamplerId, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
use crate::released::Released;
use crate::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};

pub const MAX_ACTIVE_TEXTURE_UNITS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SamplerState {
    pub filter: TextureFilterType,
    pub wrap_u: TextureWrapType,
    pub wrap_v: TextureWrapType,
    pub max_anisotropy: u8,
    pub mipmapped: bool,
}

impl Default for SamplerState {
    fn default() -> Self {
        SamplerState {
            filter: TextureFilterType::Nearest,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            max_anisotropy: 1,
            mipmapped: false,
        }
    }
}

/// The texture format of `pixel` pixels of `channel` channels that a shader
/// samples.
#[must_use]
pub fn texture_format(pixel: TexturePixelType, channel: TextureChannelDataType) -> TextureFormat {
    match (pixel, channel) {
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::UnsignedByte) => {
            TextureFormat::A8Unorm
        }
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::HalfFloat) => {
            TextureFormat::R16Float
        }
        (TexturePixelType::Alpha | TexturePixelType::Luminance, TextureChannelDataType::Float) => {
            TextureFormat::R32Float
        }
        (TexturePixelType::Depth, _) => TextureFormat::Depth32Float,
        (TexturePixelType::Stencil, _) => TextureFormat::Depth24PlusStencil8,
        (TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte) => TextureFormat::Rgba8Unorm,
        (TexturePixelType::RGBA, TextureChannelDataType::HalfFloat) => TextureFormat::Rgba16Float,
        (TexturePixelType::RGBA, TextureChannelDataType::Float) => TextureFormat::Rgba32Float,
    }
}

/// A texture of the renderer: its size, format and sampler state, the image
/// it waits to upload, and once made, a texture of the backend's
/// [`Gpu`](crate::gpu::Gpu), which goes back to the context to destroy when
/// the texture is dropped or replaced (see [`Released`]).
pub struct Texture2D {
    texture: Option<TextureId>,
    size: Size,
    format: TextureFormat,
    render_target: bool,
    sampler: SamplerState,
    /// The image the next [`upload`](Self::upload) writes.
    image: Option<Rc<PremultipliedImage>>,
    released: Rc<Released>,
}

/// A shared texture, which tweakers mutate through the shared handle (e.g.
/// its sampler state), hence the `RefCell`.
pub type Texture2DPtr = Rc<RefCell<Texture2D>>;

impl Texture2D {
    /// An RGBA texture without a size, which goes back to `released` once
    /// made.
    #[must_use]
    pub fn new(released: Rc<Released>) -> Texture2D {
        Texture2D {
            texture: None,
            size: Size::new(0, 0),
            format: TextureFormat::Rgba8Unorm,
            render_target: false,
            sampler: SamplerState::default(),
            image: None,
            released,
        }
    }

    /// The backend's texture, once made.
    #[must_use]
    pub fn id(&self) -> Option<TextureId> {
        self.texture
    }

    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }

    /// Sets the size the texture is made with. A texture made with another
    /// size goes.
    pub fn set_size(&mut self, size: Size) {
        if size != self.size {
            self.release();
            self.size = size;
        }
    }

    #[must_use]
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Sets the format the texture is made with. A texture made with
    /// another format goes.
    pub fn set_format(&mut self, format: TextureFormat) {
        if format != self.format {
            self.release();
            self.format = format;
        }
    }

    /// Makes the texture one that passes draw into, as well as one that
    /// shaders sample.
    pub fn set_render_target(&mut self) {
        self.render_target = true;
    }

    #[must_use]
    pub fn sampler(&self) -> SamplerState {
        self.sampler
    }

    /// Sets how shaders sample the texture, with an anisotropy of at least 1.
    pub fn set_sampler(&mut self, sampler: SamplerState) {
        self.sampler = SamplerState { max_anisotropy: sampler.max_anisotropy.max(1), ..sampler };
    }

    /// Sets the RGBA image the next [`upload`](Self::upload) writes, and the
    /// texture's size to the image's.
    pub fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        if let Some(image) = &image {
            self.set_size(image.size);
            self.set_format(TextureFormat::Rgba8Unorm);
        }
        self.image = image;
    }

    /// Whether an image waits for [`upload`](Self::upload).
    #[must_use]
    pub fn needs_upload(&self) -> bool {
        self.image.is_some()
    }

    /// Makes the backend's texture of the size and format set, unless it is
    /// made. Fails for an empty size.
    pub fn create(&mut self, context: &dyn Context) -> Result<TextureId, GpuError> {
        if let Some(texture) = self.texture {
            return Ok(texture);
        }
        let desc = TextureDesc {
            label: if self.render_target { "render target" } else { "texture" },
            size: self.size,
            format: self.format,
            usage: TextureUsage { sampled: true, render_target: self.render_target, copy_src: self.render_target },
        };
        let texture = context.gpu().create_texture(&desc)?;
        let mut stats = context.rendering_stats();
        stats.num_created_textures += 1;
        stats.num_active_textures += 1;
        stats.mem_textures += self.bytes() as i32;
        self.texture = Some(texture);
        Ok(texture)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture, first making it of that size. Empty `data`
    /// leaves the texels unspecified, and an empty size makes no texture.
    pub fn upload_data(&mut self, context: &dyn Context, data: &[u8], size: Size) -> Result<(), GpuError> {
        self.set_size(size);
        if size.is_empty() {
            return Ok(());
        }
        if data.is_empty() {
            return self.create(context).map(|_| ());
        }
        self.upload_sub_region(context, data, size, 0, 0)
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture, first making it.
    pub fn upload_sub_region(
        &mut self,
        context: &dyn Context,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let texture = self.create(context)?;
        let region = TextureRegion { x: u32::from(x), y: u32::from(y), size };
        context.gpu().write_texture(texture, region, data)?;
        let mut stats = context.rendering_stats();
        stats.num_texture_updates += 1;
        stats.texture_update_bytes += data.len();
        Ok(())
    }

    /// Writes the image [`set_image`](Self::set_image) set, if any.
    pub fn upload(&mut self, context: &dyn Context) -> Result<(), GpuError> {
        match self.image.take() {
            Some(image) if image.valid() => self.upload_data(context, &image.data, image.size),
            _ => Ok(()),
        }
    }

    /// What a draw binds: the texture, made and holding its image, and the
    /// sampler of its state.
    pub fn binding(&mut self, context: &dyn Context) -> Result<(TextureId, SamplerId), GpuError> {
        self.upload(context)?;
        let texture = self.create(context)?;
        Ok((texture, context.sampler(&self.sampler)?))
    }

    /// The bytes the texture takes.
    fn bytes(&self) -> usize {
        let texel = self.format.bytes_per_texel().unwrap_or(4) as usize;
        self.size.width as usize * self.size.height as usize * texel
    }

    /// Lets go of the backend's texture.
    fn release(&mut self) {
        if let Some(texture) = self.texture.take() {
            self.released.texture(texture, self.bytes());
        }
    }
}

impl Drop for Texture2D {
    fn drop(&mut self) {
        self.release();
    }
}

/// Writes a premultiplied RGBA image to `texture`, setting its format.
pub fn upload_image(
    context: &dyn Context,
    texture: &mut Texture2D,
    image: &PremultipliedImage,
) -> Result<(), GpuError> {
    texture.set_format(TextureFormat::Rgba8Unorm);
    texture.upload_data(context, &image.data, image.size)
}
