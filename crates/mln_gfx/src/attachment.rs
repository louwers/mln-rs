//! A texture that passes draw into and nothing samples, such as a target's
//! depth and stencil.

use std::rc::Rc;

use mln_util::size::Size;

use crate::context::Context;
use crate::gpu::{GpuError, TextureDesc, TextureFormat, TextureId, TextureUsage};
use crate::released::Released;

/// A render target of the context's `Gpu`, which goes to the context's
/// release queue when dropped, as a [`Texture2D`](crate::texture2d::Texture2D)
/// does. The rendering stats don't count it.
pub struct Attachment {
    id: TextureId,
    format: TextureFormat,
    size: Size,
    released: Rc<Released>,
}

impl Attachment {
    /// A render target of `size` and `format`, which readbacks read where
    /// `copy_src`.
    pub fn new(
        context: &dyn Context,
        label: &str,
        size: Size,
        format: TextureFormat,
        copy_src: bool,
    ) -> Result<Attachment, GpuError> {
        let usage = TextureUsage { render_target: true, copy_src, ..TextureUsage::default() };
        let id = context.gpu().create_texture(&TextureDesc { label, size, format, usage })?;
        Ok(Attachment { id, format, size, released: Rc::clone(&context.context_base().released) })
    }

    #[must_use]
    pub fn id(&self) -> TextureId {
        self.id
    }

    #[must_use]
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.released.attachment(self.id);
    }
}
