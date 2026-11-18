//! A texture that passes draw into and nothing samples, such as a target's
//! depth and stencil.

use std::rc::Rc;

use mln_gfx::gpu::{Gpu, GpuError, TextureDesc, TextureFormat, TextureId, TextureUsage};
use mln_gfx::released::Released;
use mln_util::size::Size;

/// A render target of the context's `Gpu`, which goes to the context's
/// release queue when dropped, as a [`Texture2D`](crate::gfx::texture2d::Texture2D)'s
/// backend texture does. The rendering stats don't count it.
pub(crate) struct Attachment {
    id: TextureId,
    format: TextureFormat,
    released: Rc<Released>,
}

impl Attachment {
    /// A render target of `size` and `format`, made through `gpu`, which
    /// goes to `released` when dropped.
    pub(crate) fn new(
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<Attachment, GpuError> {
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let id = gpu.create_texture(&TextureDesc { label, size, format, usage })?;
        Ok(Attachment { id, format, released: Rc::clone(released) })
    }

    #[must_use]
    pub(crate) fn id(&self) -> TextureId {
        self.id
    }

    #[must_use]
    pub(crate) fn format(&self) -> TextureFormat {
        self.format
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.released.attachment(self.id);
    }
}
