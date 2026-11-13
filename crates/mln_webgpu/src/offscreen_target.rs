// The offscreen target: frames drawn into a texture of the library's and
// read back, as C++'s headless backend renders (headless_backend.cpp).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::types::TextureChannelDataType;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::draw_target::{DrawTarget, downcast_resource};
use crate::offscreen_texture::OffscreenTextureResource;
use crate::renderer_backend::RendererBackend;

/// Frames drawn into a texture with depth and stencil, which
/// [`read_image`](Self::read_image) reads back.
pub struct OffscreenTarget {
    backend: Rc<RendererBackend>,
    size: Cell<Size>,
    /// Made with the first frame of a size.
    image: RefCell<Option<Rc<DrawTarget>>>,
}

impl OffscreenTarget {
    /// A target of `size` physical pixels on `backend`.
    pub fn new(backend: Rc<RendererBackend>, size: Size) -> Self {
        OffscreenTarget { backend, size: Cell::new(size), image: RefCell::new(None) }
    }

    /// The backend the target draws on.
    #[cfg(test)]
    pub(crate) fn backend(&self) -> &Rc<RendererBackend> {
        &self.backend
    }

    /// Draws the next frame at `size`, and drops the image of the last one.
    pub fn resize(&self, size: Size) {
        if self.size.replace(size) != size {
            self.image.borrow_mut().take();
        }
    }

    /// The last frame's pixels, or `None` before the first frame of the
    /// size.
    pub fn read_image(&self) -> Option<PremultipliedImage> {
        let image = self.image.borrow();
        let resource = downcast_resource::<OffscreenTextureResource>(&**image.as_ref()?)?;
        Some(resource.read_still_image())
    }
}

impl FrameTarget for OffscreenTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let resource = OffscreenTextureResource::new(
                self.backend.webgpu_context()?,
                size,
                TextureChannelDataType::UnsignedByte,
                true,
                true,
            );
            *image = Some(Rc::new(DrawTarget::new(size, Rc::new(resource))));
        }
        image.clone().map(|image| image as Rc<dyn frame::DrawTarget>)
    }

    fn end_frame(&self, encoder: &dyn GfxCommandEncoder) -> Handoff {
        let image = self.image.borrow();
        if let Some(encoder) = encoder.downcast_ref::<CommandEncoder>() {
            encoder.submit_frame(image.as_deref().map(DrawTarget::resource));
        }
        if image.is_some() { Handoff::ReadBack } else { Handoff::Dropped }
    }
}
