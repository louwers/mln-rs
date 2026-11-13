// The target of a browser canvas: a WebGL2 context, which is OpenGL ES 3.0,
// on an OffscreenCanvas. After the GLFW app's backend
// (platform/glfw/glfw_gl_backend.cpp), whose window is the canvas here.
//
// The context belongs to the canvas and is always current, and the browser
// presents the canvas after the task that drew into it, so there is nothing
// to activate or swap.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_util::error::Error;
use mln_util::size::Size;
use wasm_bindgen::JsCast;
use web_sys::{OffscreenCanvas, WebGl2RenderingContext, WebGlContextAttributes};

use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::{PlatformContext, RendererBackend};

/// The canvas's context, which is always current.
struct CanvasContext;

impl PlatformContext for CanvasContext {
    fn make_current(&self) {}
    fn done_current(&self) {}
}

/// The canvas's drawing buffer: the default framebuffer.
struct CanvasResource {
    context: Weak<Context>,
    size: Rc<Cell<Size>>,
}

impl TargetResource for CanvasResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(None, self.size.get(), true);
        }
    }
}

/// Frames drawn into a canvas, which the browser presents.
pub struct CanvasTarget {
    backend: Rc<RendererBackend>,
    canvas: OffscreenCanvas,
    size: Rc<Cell<Size>>,
    /// Made with the first frame.
    image: RefCell<Option<Rc<DrawTarget>>>,
}

impl CanvasTarget {
    /// A backend on a WebGL2 context of `canvas`, and a target that draws
    /// into the canvas, whose drawing buffer becomes `size` pixels. The
    /// drawing buffer has a depth and a stencil buffer and no antialiasing,
    /// and its colors are premultiplied by their alpha, as the map draws
    /// them.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// browser makes no WebGL2 context, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it throws.
    pub fn with_backend(canvas: OffscreenCanvas, size: Size) -> Result<(Rc<RendererBackend>, CanvasTarget), Error> {
        canvas.set_width(size.width);
        canvas.set_height(size.height);
        let attributes = WebGlContextAttributes::new();
        attributes.set_alpha(true);
        attributes.set_premultiplied_alpha(true);
        attributes.set_antialias(false);
        attributes.set_depth(true);
        attributes.set_stencil(true);
        let webgl2 = canvas
            .get_context_with_context_options("webgl2", &attributes)
            .map_err(|error| Error::native(format!("{error:?}")))?
            .and_then(|context| context.dyn_into::<WebGl2RenderingContext>().ok())
            .ok_or_else(|| Error::unsupported("the canvas has no WebGL2 context"))?;
        // WebGL2 renders into float textures once this is asked for (the
        // heatmap's half-float one).
        let _ = webgl2.get_extension("EXT_color_buffer_float");
        let backend = Rc::new(RendererBackend::from_parts(webgl2, Box::new(CanvasContext)));
        let target = CanvasTarget {
            backend: backend.clone(),
            canvas,
            size: Rc::new(Cell::new(size)),
            image: RefCell::new(None),
        };
        Ok((backend, target))
    }

    /// The size of the frames, in physical pixels.
    pub fn size(&self) -> Size {
        self.size.get()
    }

    /// Resizes the canvas's drawing buffer.
    pub fn resize(&self, size: Size) {
        if size == self.size.replace(size) {
            return;
        }
        self.canvas.set_width(size.width);
        self.canvas.set_height(size.height);
    }
}

impl FrameTarget for CanvasTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let resource = CanvasResource { context: self.backend.gl_context()?.weak(), size: self.size.clone() };
            *image = Some(Rc::new(DrawTarget::new(self.size.get(), Rc::new(resource))));
        }
        let image = image.clone()?;
        image.set_size(self.size.get());
        Some(image)
    }

    /// The browser presents the canvas after the task that drew into it.
    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        Handoff::Presented
    }
}
