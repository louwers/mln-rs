//! A texture that passes draw into and nothing samples, such as a target's
//! depth and stencil.

use mln_gfx::gpu::{Gpu, GpuError, TextureDesc, TextureFormat, TextureId, TextureUsage};
use mln_util::size::Size;

use crate::gfx::resources::{Resources, SharedGpuTexture};

/// A render target of the context's `Gpu`, which the context destroys once
/// this drops, as a [`Texture2D`]'s backend texture. The rendering stats
/// don't count it.
///
/// [`Texture2D`]: crate::gfx::texture2d::Texture2D
pub(crate) struct Attachment {
    texture: SharedGpuTexture,
    format: TextureFormat,
}

impl Attachment {
    /// A render target of `size` and `format`, made through `gpu` into
    /// `resources`.
    pub(crate) fn new(
        gpu: &mut dyn Gpu,
        resources: &mut Resources,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<Attachment, GpuError> {
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let texture = resources.texture(gpu, &TextureDesc { label, size, format, usage }, None)?;
        Ok(Attachment { texture, format })
    }

    #[must_use]
    pub(crate) fn id(&self) -> TextureId {
        self.texture.id()
    }

    #[must_use]
    pub(crate) fn format(&self) -> TextureFormat {
        self.format
    }
}
