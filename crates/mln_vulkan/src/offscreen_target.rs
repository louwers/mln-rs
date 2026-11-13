// The offscreen target: frames drawn into images of the library's and read
// back, as C++'s headless backend renders (headless_backend.cpp), which
// keeps a copy of every frame to read (`HeadlessRenderableResource`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ash::vk;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::draw_target::DrawTarget;
use crate::renderer_backend::RendererBackend;
use crate::surface_resource::SurfaceResource;

/// Frames drawn into images with depth and stencil, which
/// [`read_image`](Self::read_image) reads back.
pub struct OffscreenTarget {
    backend: Rc<RendererBackend>,
    size: Cell<Size>,
    /// Made with the first frame of a size.
    image: RefCell<Option<(Rc<SurfaceResource>, Rc<DrawTarget>)>>,
}

impl OffscreenTarget {
    /// A target of `size` physical pixels on `backend`.
    pub fn new(backend: Rc<RendererBackend>, size: Size) -> Self {
        OffscreenTarget { backend, size: Cell::new(size), image: RefCell::new(None) }
    }

    /// Draws the next frame at `size`, and drops the images of the last one.
    pub fn resize(&self, size: Size) {
        if self.size.replace(size) != size {
            self.image.borrow_mut().take();
        }
    }

    /// The last frame's pixels, or `None` before the first frame of the
    /// size.
    pub fn read_image(&self) -> Option<PremultipliedImage> {
        self.image.borrow().as_ref()?.0.read_image()
    }
}

impl FrameTarget for OffscreenTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let context = self.backend.vulkan_context();
        let mut image = self.image.borrow_mut();
        let (resource, target) = image.get_or_insert_with(|| {
            let resource = SurfaceResource::new(
                context,
                size,
                None,
                None,
                vk::PresentModeKHR::FIFO,
                /*read_every_frame=*/ true,
            );
            let target = Rc::new(DrawTarget::new(size, Rc::<SurfaceResource>::clone(&resource)));
            (resource, target)
        });
        context.set_surface(resource);
        Some(Rc::<DrawTarget>::clone(target))
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        match self.image.borrow().as_ref() {
            Some((resource, _)) => {
                resource.swap();
                Handoff::ReadBack
            }
            None => Handoff::Dropped,
        }
    }
}
