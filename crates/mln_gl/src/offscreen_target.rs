use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{BackendScope, ScopeType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource, downcast_resource};
use crate::renderer_backend::RendererBackend;
use crate::types::{FramebufferID, RenderbufferID};

/// A framebuffer of a color renderbuffer and a depth and stencil
/// renderbuffer.
struct OffscreenResource {
    context: Weak<Context>,
    size: Size,
    color: RenderbufferID,
    depth_stencil: RenderbufferID,
    framebuffer: FramebufferID,
}

impl OffscreenResource {
    fn new(context: &Context, size: Size) -> Option<Self> {
        let gl = context.gl();
        // SAFETY: the renderbuffers and the framebuffer are new.
        unsafe {
            let color = gl.create_renderbuffer().ok()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, size.width as i32, size.height as i32);
            let depth_stencil = gl.create_renderbuffer().ok()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth_stencil));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, size.width as i32, size.height as i32);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let framebuffer = gl.create_framebuffer().ok()?;
            context.state().bind_framebuffer(gl, Some(framebuffer));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(color));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(depth_stencil),
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                Log::error(Event::GraphicsBackend, format!("Couldn't create framebuffer: status {status:#x}"));
            }
            mln_gfx::context::Context::rendering_stats(context).num_frame_buffers += 1;
            Some(OffscreenResource { context: context.weak(), size, color, depth_stencil, framebuffer })
        }
    }

    fn read_still_image(&self) -> PremultipliedImage {
        let mut image = PremultipliedImage::new(self.size);
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(Some(self.framebuffer), self.size, true);
            image.data = context.read_framebuffer(self.size);
        }
        image
    }
}

impl Drop for OffscreenResource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.abandon_framebuffer(self.framebuffer);
            context.abandon_renderbuffer(self.color);
            context.abandon_renderbuffer(self.depth_stencil);
            mln_gfx::context::Context::rendering_stats(&*context).num_frame_buffers -= 1;
        }
    }
}

impl TargetResource for OffscreenResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(Some(self.framebuffer), self.size, true);
        }
    }
}

/// Frames drawn into a framebuffer of a color renderbuffer and a depth and
/// stencil one, which [`read_image`](Self::read_image) reads back.
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

    /// Draws the next frame at `size`, and drops the framebuffer of the last
    /// one.
    pub fn resize(&self, size: Size) {
        if self.size.replace(size) != size {
            self.image.borrow_mut().take();
        }
    }

    /// The last frame's pixels, or `None` before the first frame of the
    /// size.
    pub fn read_image(&self) -> Option<PremultipliedImage> {
        let _scope = BackendScope::new(&*self.backend, ScopeType::Explicit);
        let image = self.image.borrow();
        Some(downcast_resource::<OffscreenResource>(&**image.as_ref()?)?.read_still_image())
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
            let resource = OffscreenResource::new(&*self.backend.gl_context()?, size)?;
            *image = Some(Rc::new(DrawTarget::new(size, Rc::new(resource))));
        }
        image.clone().map(|image| image as Rc<dyn frame::DrawTarget>)
    }

    /// The frame is read back when it is read: GL runs the commands in the
    /// order they come.
    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.image.borrow().is_some() { Handoff::ReadBack } else { Handoff::Dropped }
    }
}
