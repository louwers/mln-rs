//! The renderer's textures. Each is a [`Texture2D`] in the context's store,
//! which one owner holds as an [`OwnedTexture`] and draws name by its
//! [`TextureKey`].

use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::gpu::{Gpu, GpuError, SamplerId, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
use mln_gfx::released::Released;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::texture2d::SamplerState;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

slotmap::new_key_type! {
    /// A texture of the context's store, as draws name it.
    pub(crate) struct TextureKey;
}

/// The textures whose owners dropped them, which the context takes out of
/// its store the next time it destroys what the renderer released.
#[derive(Default)]
pub(crate) struct DroppedTextures(Cell<Vec<TextureKey>>);

impl DroppedTextures {
    fn push(&self, key: TextureKey) {
        let mut keys = self.0.take();
        keys.push(key);
        self.0.set(keys);
    }

    /// The textures dropped so far, lent until `restore` or
    /// `restore_empty`.
    pub(crate) fn take(&self) -> Vec<TextureKey> {
        self.0.take()
    }

    /// Gives back what `take` lent, ahead of what dropped since.
    pub(crate) fn restore(&self, mut keys: Vec<TextureKey>) {
        keys.append(&mut self.0.take());
        self.0.set(keys);
    }

    /// Gives back the memory of what `take` lent, without the keys.
    pub(crate) fn restore_empty(&self, mut keys: Vec<TextureKey>) {
        keys.clear();
        self.restore(keys);
    }
}

/// The owner of a texture of the context's store, which goes from the store
/// once this drops.
pub(crate) struct OwnedTexture {
    key: TextureKey,
    dropped: Rc<DroppedTextures>,
}

impl OwnedTexture {
    pub(crate) fn new(key: TextureKey, dropped: Rc<DroppedTextures>) -> OwnedTexture {
        OwnedTexture { key, dropped }
    }

    /// The texture's key, which draws name it by.
    #[must_use]
    pub(crate) fn key(&self) -> TextureKey {
        self.key
    }
}

impl Drop for OwnedTexture {
    fn drop(&mut self) {
        self.dropped.push(self.key);
    }
}

/// A texture of the renderer: its size, format and sampler state, the image
/// it waits to upload, and once made, a texture of the backend's
/// [`Gpu`](mln_gfx::gpu::Gpu), which goes back to the context to destroy when
/// the texture is dropped or replaced (see [`Released`]).
pub(crate) struct Texture2D {
    texture: Option<TextureId>,
    size: Size,
    format: TextureFormat,
    render_target: bool,
    sampler: SamplerState,
    /// The context's sampler of `sampler`, once a draw bound it.
    sampler_id: Option<SamplerId>,
    /// The image the next upload writes.
    image: Option<Rc<PremultipliedImage>>,
    released: Rc<Released>,
}

impl Texture2D {
    /// An RGBA texture without a size, which goes back to `released` once
    /// made.
    #[must_use]
    pub(crate) fn new(released: Rc<Released>) -> Texture2D {
        Texture2D {
            texture: None,
            size: Size::new(0, 0),
            format: TextureFormat::Rgba8Unorm,
            render_target: false,
            sampler: SamplerState::default(),
            sampler_id: None,
            image: None,
            released,
        }
    }

    #[must_use]
    pub(crate) fn size(&self) -> Size {
        self.size
    }

    /// Sets the size the texture is made with. A texture made with another
    /// size goes.
    pub(crate) fn set_size(&mut self, size: Size) {
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
    pub(crate) fn set_format(&mut self, format: TextureFormat) {
        if format != self.format {
            self.release();
            self.format = format;
        }
    }

    /// Makes the texture one that passes draw into, as well as one that
    /// shaders sample.
    pub(crate) fn set_render_target(&mut self) {
        self.render_target = true;
    }

    /// How shaders sample the texture.
    #[must_use]
    pub(crate) fn sampler(&self) -> SamplerState {
        self.sampler
    }

    /// Sets how shaders sample the texture, with an anisotropy of at least 1.
    pub(crate) fn set_sampler(&mut self, sampler: SamplerState) {
        let sampler = SamplerState { max_anisotropy: sampler.max_anisotropy.max(1), ..sampler };
        if sampler != self.sampler {
            self.sampler = sampler;
            self.sampler_id = None;
        }
    }

    /// The context's sampler of the texture's sampler state, once a draw
    /// bound it.
    pub(crate) fn sampler_id(&self) -> Option<SamplerId> {
        self.sampler_id
    }

    pub(crate) fn set_sampler_id(&mut self, sampler: SamplerId) {
        self.sampler_id = Some(sampler);
    }

    /// Sets the RGBA image the next upload writes, and the texture's size to
    /// the image's.
    pub(crate) fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        if let Some(image) = &image {
            self.set_size(image.size);
            self.set_format(TextureFormat::Rgba8Unorm);
        }
        self.image = image;
    }

    /// Whether the texture has an image to upload.
    pub(crate) fn has_image(&self) -> bool {
        self.image.is_some()
    }

    /// Makes the backend's texture of the size and format set through `gpu`,
    /// unless it is made, counting it in `stats`. Fails for an empty size.
    pub(crate) fn create(&mut self, gpu: &mut dyn Gpu, stats: &mut RenderingStats) -> Result<TextureId, GpuError> {
        if let Some(texture) = self.texture {
            return Ok(texture);
        }
        let desc = TextureDesc {
            label: if self.render_target { "render target" } else { "texture" },
            size: self.size,
            format: self.format,
            usage: TextureUsage { sampled: true, render_target: self.render_target, copy_src: self.render_target },
        };
        let texture = gpu.create_texture(&desc)?;
        stats.num_created_textures += 1;
        stats.num_active_textures += 1;
        stats.mem_textures += self.bytes() as i32;
        self.texture = Some(texture);
        Ok(texture)
    }

    /// Writes `data`, the texels of a texture of `size` in rows from the top,
    /// to the whole texture, first making it of that size. Empty `data`
    /// leaves the texels unspecified, and an empty size makes no texture.
    pub(crate) fn upload_data(
        &mut self,
        gpu: &mut dyn Gpu,
        stats: &mut RenderingStats,
        data: &[u8],
        size: Size,
    ) -> Result<(), GpuError> {
        self.set_size(size);
        if size.is_empty() {
            return Ok(());
        }
        let texture = self.create(gpu, stats)?;
        if data.is_empty() {
            return Ok(());
        }
        write(gpu, stats, texture, data, TextureRegion { x: 0, y: 0, size })
    }

    /// Writes `data`, the texels of a region of `size` in rows from the top,
    /// at `x`, `y` of the texture, first making it.
    pub(crate) fn upload_sub_region(
        &mut self,
        gpu: &mut dyn Gpu,
        stats: &mut RenderingStats,
        data: &[u8],
        size: Size,
        x: u16,
        y: u16,
    ) -> Result<(), GpuError> {
        let texture = self.create(gpu, stats)?;
        write(gpu, stats, texture, data, TextureRegion { x: u32::from(x), y: u32::from(y), size })
    }

    /// Writes the image [`set_image`](Self::set_image) set, if any.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, stats: &mut RenderingStats) -> Result<(), GpuError> {
        match self.image.take() {
            Some(image) if image.valid() => self.upload_data(gpu, stats, &image.data, image.size),
            _ => Ok(()),
        }
    }

    /// The texture a draw binds, made through `gpu` and holding its image.
    pub(crate) fn bind(&mut self, gpu: &mut dyn Gpu, stats: &mut RenderingStats) -> Result<TextureId, GpuError> {
        self.upload(gpu, stats)?;
        self.create(gpu, stats)
    }

    /// Takes the texture, if it is made, out of `stats`, as its release
    /// does.
    pub(crate) fn uncount(&self, stats: &mut RenderingStats) {
        if self.texture.is_some() {
            stats.num_active_textures -= 1;
            stats.mem_textures -= self.bytes() as i32;
        }
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
