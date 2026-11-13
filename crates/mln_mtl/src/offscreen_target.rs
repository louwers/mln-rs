// The offscreen target: frames drawn into a texture of the library's and
// read back, as C++'s headless backend renders (headless_backend.cpp).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::types::TextureChannelDataType;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use objc2_metal::MTLCommandBuffer as _;

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

    pub fn size(&self) -> Size {
        self.size.get()
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
                self.backend.mtl_context()?,
                size,
                TextureChannelDataType::UnsignedByte,
                true,
                true,
            );
            *image = Some(Rc::new(DrawTarget::new(size, Rc::new(resource))));
        }
        image.clone().map(|image| image as Rc<dyn frame::DrawTarget>)
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        let image = self.image.borrow();
        match image
            .as_deref()
            .and_then(|image| (image.resource() as &dyn Any).downcast_ref::<OffscreenTextureResource>())
        {
            Some(resource) => {
                // Nothing presents the frame, so waiting for it here is what
                // paces continuous rendering.
                if let Some(command_buffer) = resource.commit() {
                    command_buffer.waitUntilCompleted();
                }
                Handoff::ReadBack
            }
            None => Handoff::Dropped,
        }
    }
}
