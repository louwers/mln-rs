//! A texture that passes draw into and nothing samples, such as a target's
//! depth and stencil.

use std::rc::Rc;

use mln_gfx::gpu::{GpuError, TextureDesc, TextureFormat, TextureId, TextureUsage};
use mln_gfx::released::Released;
use mln_util::size::Size;

use crate::gfx::context::Context;

/// A render target of the context's `Gpu`, which goes to the context's
/// release queue when dropped, as a [`Texture2D`](crate::gfx::texture2d::Texture2D)
/// does. The rendering stats don't count it.
pub(crate) struct Attachment {
    id: TextureId,
    format: TextureFormat,
    released: Rc<Released>,
}

impl Attachment {
    /// A render target of `size` and `format`.
    pub(crate) fn new(
        context: &Context,
        label: &str,
        size: Size,
        format: TextureFormat,
    ) -> Result<Attachment, GpuError> {
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let id = context.gpu().create_texture(&TextureDesc { label, size, format, usage })?;
        Ok(Attachment { id, format, released: Rc::clone(context.released()) })
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
