// The target of a browser canvas: a WebGL2 context, which is OpenGL ES 3.0,
// on an OffscreenCanvas or an HTMLCanvasElement.
//
// The context belongs to the canvas and is always current, and the browser
// presents the canvas after the task that drew into it, so there is nothing
// to activate or swap.

use std::cell::Cell;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_util::error::Error;
use mln_util::size::Size;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlCanvasElement, OffscreenCanvas, WebGl2RenderingContext, WebGlContextAttributes};

use crate::frame_image::{FrameImports, default_framebuffer};
use crate::gpu::GlGpu;
use crate::renderer_backend::{PlatformContext, RendererBackend};

/// The canvas's context, which is always current. The backends on it, one
/// for each session that presents to the canvas, take turns in it: the
/// context holds the number of the backend that used it last, and a backend
/// that finds another's there has lost the GL state it cached.
struct CanvasContext {
    webgl2: WebGl2RenderingContext,
    /// The backend's number, which no other backend on its thread has.
    user: u32,
}

/// The property of a context that holds the number of the backend that used
/// the context last.
const LAST_USER: &str = "mlnLastUser";

impl CanvasContext {
    fn new(webgl2: WebGl2RenderingContext) -> CanvasContext {
        thread_local! {
            static USERS: Cell<u32> = const { Cell::new(0) };
        }
        let user = USERS.with(|users| {
            users.set(users.get() + 1);
            users.get()
        });
        CanvasContext { webgl2, user }
    }
}

impl PlatformContext for CanvasContext {
    fn make_current(&mut self) {}
    fn done_current(&mut self) {}

    fn take_state_lost(&mut self) -> bool {
        let key = JsValue::from_str(LAST_USER);
        let user = JsValue::from(self.user);
        if js_sys::Reflect::get(&self.webgl2, &key).is_ok_and(|last| last == user) {
            return false;
        }
        let _ = js_sys::Reflect::set(&self.webgl2, &key, &user);
        true
    }
}

/// A canvas that a [`CanvasTarget`] draws into.
#[derive(Clone, Debug)]
pub enum WebCanvas {
    /// A canvas off the page, such as one that the page transferred to a Web
    /// Worker.
    Offscreen(OffscreenCanvas),
    /// A canvas element of the page.
    Element(HtmlCanvasElement),
}

impl From<OffscreenCanvas> for WebCanvas {
    fn from(canvas: OffscreenCanvas) -> WebCanvas {
        WebCanvas::Offscreen(canvas)
    }
}

impl From<HtmlCanvasElement> for WebCanvas {
    fn from(canvas: HtmlCanvasElement) -> WebCanvas {
        WebCanvas::Element(canvas)
    }
}

impl WebCanvas {
    fn set_size(&self, size: Size) {
        match self {
            WebCanvas::Offscreen(canvas) => {
                canvas.set_width(size.width);
                canvas.set_height(size.height);
            }
            WebCanvas::Element(canvas) => {
                canvas.set_width(size.width);
                canvas.set_height(size.height);
            }
        }
    }

    fn context(&self, attributes: &WebGlContextAttributes) -> Result<Option<js_sys::Object>, JsValue> {
        match self {
            WebCanvas::Offscreen(canvas) => canvas.get_context_with_context_options("webgl2", attributes),
            WebCanvas::Element(canvas) => canvas.get_context_with_context_options("webgl2", attributes),
        }
    }
}

/// Frames drawn into a canvas, which the browser presents.
pub struct CanvasTarget {
    canvas: WebCanvas,
    /// The size of the next frame.
    size: Size,
    /// The size of the canvas's drawing buffer.
    canvas_size: Size,
    /// The frame's default framebuffer, imported into the backend's `GlGpu`.
    image: FrameImports,
}

impl CanvasTarget {
    /// A backend on the WebGL2 context of `canvas`, which the canvas makes
    /// unless it has one, and a target that draws into the canvas, whose
    /// drawing buffer becomes `size` pixels. Backends on one canvas share
    /// its context, and each draws whole frames into it. The
    /// drawing buffer has a depth and a stencil buffer and no antialiasing,
    /// and its colors are premultiplied by their alpha, as the map draws
    /// them.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// browser makes no WebGL2 context, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it throws.
    pub fn with_backend(canvas: impl Into<WebCanvas>, size: Size) -> Result<(RendererBackend, CanvasTarget), Error> {
        let canvas = canvas.into();
        canvas.set_size(size);
        let attributes = WebGlContextAttributes::new();
        attributes.set_alpha(true);
        attributes.set_premultiplied_alpha(true);
        attributes.set_antialias(false);
        attributes.set_depth(true);
        attributes.set_stencil(true);
        let webgl2 = canvas
            .context(&attributes)
            .map_err(|error| Error::native(format!("{error:?}")))?
            .and_then(|context| context.dyn_into::<WebGl2RenderingContext>().ok())
            .ok_or_else(|| Error::unsupported("the canvas has no WebGL2 context"))?;
        // WebGL2 renders into float textures once this is asked for (the
        // heatmap's half-float one).
        let _ = webgl2.get_extension("EXT_color_buffer_float");
        let platform = CanvasContext::new(webgl2.clone());
        let backend = RendererBackend::from_parts(webgl2, Box::new(platform));
        Ok((backend, CanvasTarget { canvas, size, canvas_size: size, image: FrameImports::default() }))
    }
}

impl FrameTarget for CanvasTarget {
    type Gpu = GlGpu;

    /// Draws the next frame at `size` pixels. The canvas's drawing buffer
    /// takes the size when that frame begins: resizing it clears it, and the
    /// browser would show it cleared after any task that resized it without
    /// drawing.
    fn resize(&mut self, _gpu: &mut GlGpu, size: Size) {
        self.size = size;
    }

    fn begin_frame(&mut self, gpu: &mut GlGpu) -> Option<FrameImage> {
        let size = self.size;
        if size != std::mem::replace(&mut self.canvas_size, size) {
            self.canvas.set_size(size);
        }
        self.image.get_or_import(|| default_framebuffer(gpu, size))
    }

    /// The browser presents the canvas after the task that drew into it.
    fn end_frame(&mut self, gpu: &mut GlGpu) -> Handoff {
        self.image.release(gpu);
        Handoff::Presented
    }

    fn tear_down(&mut self, gpu: &mut GlGpu) {
        self.image.release(gpu);
    }
}
