// The backend of a browser canvas: a WebGL2 context, which is OpenGL ES 3.0,
// on an OffscreenCanvas. After the GLFW app's backend
// (platform/glfw/glfw_gl_backend.cpp), whose window is the canvas here.
//
// The context belongs to the canvas and is always current, and the browser
// presents the canvas after the task that drew into it, so there is nothing
// to activate or swap.

use std::any::Any;
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderable::{Renderable, RenderableBase};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::error::Error;
use mln_util::size::Size;
use wasm_bindgen::JsCast;
use web_sys::{OffscreenCanvas, WebGl2RenderingContext, WebGlContextAttributes};

use crate::context::Context;
use crate::renderable_resource::{RenderableResource, RenderableResourceBox};
use crate::renderer_backend::{PlatformContext, RendererBackend};

/// The canvas's context, which is always current.
struct CanvasContext;

impl PlatformContext for CanvasContext {
    fn make_current(&self) {}
    fn done_current(&self) {}
}

/// The canvas's drawing buffer: the default framebuffer.
struct CanvasRenderableResource {
    context: Weak<Context>,
    size: Size,
}

impl RenderableResource for CanvasRenderableResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(None, self.size, true);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A `gl::RendererBackend` that renders into a canvas.
pub struct CanvasBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
    canvas: OffscreenCanvas,
}

impl CanvasBackend {
    /// A WebGL2 context on `canvas`, whose drawing buffer becomes `size`
    /// pixels, or why the browser made none. The drawing buffer has a depth
    /// and a stencil buffer and no antialiasing, and its colors are
    /// premultiplied by their alpha, as the map draws them.
    pub fn new(canvas: OffscreenCanvas, size: Size, mode: ContextMode) -> Result<Self, Error> {
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
        Ok(CanvasBackend {
            renderer_backend: RendererBackend::new(mode, webgl2, Box::new(CanvasContext)),
            renderable: RenderableBase::new(size, None),
            canvas,
        })
    }

    pub fn gl_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// `GL_RENDERER`: the name of the GL implementation.
    pub fn get_renderer_name(&self) -> String {
        // SAFETY: the context is current.
        unsafe { self.renderer_backend.gl().get_parameter_string(glow::RENDERER) }
    }

    pub fn get_framebuffer_size(&self) -> Size {
        self.get_size()
    }

    /// Resizes the canvas's drawing buffer.
    pub fn set_framebuffer_size(&self, size: Size) {
        if size == self.get_size() {
            return;
        }
        self.canvas.set_width(size.width);
        self.canvas.set_height(size.height);
        self.set_renderable_size(size);
        self.set_resource(None);
    }
}

impl Drop for CanvasBackend {
    fn drop(&mut self) {
        // The renderable resource goes before the context.
        self.set_resource(None);
        self.renderer_backend.reset_context();
    }
}

impl Renderable for CanvasBackend {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for CanvasBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.renderer_backend.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    /// `getDefaultRenderable()`: creates the resource first.
    fn get_default_renderable(&self) -> &dyn Renderable {
        if !self.has_resource()
            && let Some(context) =
                <Self as renderer_backend::RendererBackend>::get_context(self).as_any().downcast_ref::<Context>()
        {
            self.set_resource(Some(RenderableResourceBox::new_rc(CanvasRenderableResource {
                context: context.weak(),
                size: self.get_size(),
            })));
        }
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.renderer_backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        self.renderer_backend.create_context()
    }

    fn activate(&self) {
        self.renderer_backend.activate();
    }

    fn deactivate(&self) {
        self.renderer_backend.deactivate();
    }
}
