// Ports platform/default/include/mln/gl/headless_backend.hpp,
// platform/default/src/mln/gl/headless_backend.cpp and the EGL context of
// platform/linux/src/headless_backend_egl.cpp, plus
// `Backend::Create<Backend::Type::OpenGL>`.
//
// Differences from C++:
// - The EGL library is loaded at run time (egl.rs), and its display is Mesa's
//   surfaceless platform where the library has it, so that no window system
//   is needed; elsewhere it is the default display, as in C++.
// - The context is OpenGL ES 3.0 on every platform.

use std::any::Any;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderable::{Renderable, RenderableBase, SwapBehaviour};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::{BackendScope, ScopeType, headless_backend};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::Context;
use crate::egl::{Egl, library};
use crate::renderable_resource::{RenderableResource, RenderableResourceBox, downcast_resource};
use crate::renderer_backend::{PlatformContext, RendererBackend};
use crate::types::{FramebufferID, RenderbufferID};

/// `EGL_PLATFORM_SURFACELESS_MESA` (EGL_MESA_platform_surfaceless).
const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

/// The EGL library and its display (`EGLDisplayConfig`), which every
/// headless backend of the process shares. The display stays initialized.
struct EglDisplay {
    egl: &'static Egl,
    display: egl::Display,
    config: egl::Config,
}

// SAFETY: the display and the config are handles EGL lets every thread use;
// the contexts made on them are used on the thread that made them.
unsafe impl Send for EglDisplay {}
// SAFETY: as above.
unsafe impl Sync for EglDisplay {}

impl EglDisplay {
    fn get() -> Result<&'static EglDisplay, String> {
        static DISPLAY: OnceLock<Result<EglDisplay, String>> = OnceLock::new();
        DISPLAY.get_or_init(Self::open).as_ref().map_err(Clone::clone)
    }

    fn open() -> Result<EglDisplay, String> {
        let egl = library()?;
        let client_extensions =
            egl.query_string(None, egl::EXTENSIONS).ok().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let surfaceless = client_extensions.split(' ').any(|e| e == "EGL_MESA_platform_surfaceless");
        // SAFETY: the default native display is valid on every platform.
        let display = unsafe {
            match egl.upcast::<egl::EGL1_5>() {
                Some(egl15) if surfaceless => egl15
                    .get_platform_display(PLATFORM_SURFACELESS_MESA, egl::DEFAULT_DISPLAY, &[egl::ATTRIB_NONE])
                    .ok(),
                _ => None,
            }
            .or_else(|| egl.get_display(egl::DEFAULT_DISPLAY))
        }
        .ok_or("Failed to obtain a valid EGL display.")?;
        egl.initialize(display).map_err(|e| format!("eglInitialize() failed: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("Error setting the EGL rendering API: {e}"))?;
        // Note: we're choosing an arbitrary pixel format, since we're not
        // using the default surface anyway; all rendering will be directed to
        // framebuffer objects.
        let attributes = [
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &attributes)
            .map_err(|e| format!("Failed to choose an EGL config: {e}"))?
            .ok_or("Failed to choose an EGL config.")?;
        Ok(EglDisplay { egl, display, config })
    }
}

/// `EGLBackendImpl`: an OpenGL ES 3.0 context, and the 1x1 pbuffer it is
/// current on.
struct EglContext {
    egl_display: &'static EglDisplay,
    context: egl::Context,
    surface: egl::Surface,
}

impl EglContext {
    fn new() -> Result<Self, String> {
        let egl_display = EglDisplay::get()?;
        let EglDisplay { egl, display, config } = egl_display;
        let context = egl
            .create_context(*display, *config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("Error creating the EGL context object: {e}"))?;
        // Create a dummy pbuffer. We will render to framebuffers anyway, but
        // we need a pbuffer to activate the context.
        let surface = match egl.create_pbuffer_surface(*display, *config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]) {
            Ok(surface) => surface,
            Err(e) => {
                let _ = egl.destroy_context(*display, context);
                return Err(format!("Could not create surface: {e}"));
            }
        };
        Ok(EglContext { egl_display, context, surface })
    }
}

impl PlatformContext for EglContext {
    fn make_current(&self) {
        let EglDisplay { egl, display, .. } = self.egl_display;
        if let Err(e) = egl.make_current(*display, Some(self.surface), Some(self.surface), Some(self.context)) {
            Log::error(Event::GraphicsBackend, format!("Switching OpenGL context failed: {e}"));
        }
    }

    fn done_current(&self) {
        let EglDisplay { egl, display, .. } = self.egl_display;
        if let Err(e) = egl.make_current(*display, None, None, None) {
            Log::error(Event::GraphicsBackend, format!("Removing OpenGL context failed: {e}"));
        }
    }
}

impl Drop for EglContext {
    fn drop(&mut self) {
        let EglDisplay { egl, display, .. } = self.egl_display;
        if let Err(e) = egl.destroy_context(*display, self.context) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL context: {e}"));
        }
        if let Err(e) = egl.destroy_surface(*display, self.surface) {
            Log::error(Event::GraphicsBackend, format!("Failed to destroy EGL surface: {e}"));
        }
    }
}

/// `gl::HeadlessRenderableResource`: a framebuffer of a color renderbuffer
/// and a depth and stencil renderbuffer.
struct HeadlessRenderableResource {
    context: Weak<Context>,
    size: Size,
    swap_behaviour: SwapBehaviour,
    color: RenderbufferID,
    depth_stencil: RenderbufferID,
    framebuffer: FramebufferID,
}

impl HeadlessRenderableResource {
    fn new(context: &Context, size: Size, swap_behaviour: SwapBehaviour) -> Option<Self> {
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
            Some(HeadlessRenderableResource {
                context: context.weak(),
                size,
                swap_behaviour,
                color,
                depth_stencil,
                framebuffer,
            })
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

impl Drop for HeadlessRenderableResource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.abandon_framebuffer(self.framebuffer);
            context.abandon_renderbuffer(self.color);
            context.abandon_renderbuffer(self.depth_stencil);
            mln_gfx::context::Context::rendering_stats(&*context).num_frame_buffers -= 1;
        }
    }
}

impl RenderableResource for HeadlessRenderableResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(Some(self.framebuffer), self.size, true);
        }
    }

    /// `HeadlessBackend::swap()`
    fn swap(&self) {
        if self.swap_behaviour == SwapBehaviour::Flush
            && let Some(context) = self.context.upgrade()
        {
            // SAFETY: no preconditions.
            unsafe { context.gl().finish() };
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `gl::HeadlessBackend`: a `gl::RendererBackend` that renders into a
/// framebuffer of an EGL context.
pub struct HeadlessBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
    swap_behaviour: SwapBehaviour,
}

impl HeadlessBackend {
    /// `HeadlessBackend(size = {256, 256}, swapBehaviour = NoFlush,
    /// contextMode = Unique)`, or an error if there is no EGL to make an
    /// OpenGL ES 3.0 context with.
    pub fn new(size: Size, swap_behaviour: SwapBehaviour, mode: ContextMode) -> Result<Self, Error> {
        let egl_context = EglContext::new().map_err(Error::native)?;
        egl_context.make_current();
        let egl = &egl_context.egl_display.egl;
        // SAFETY: the context is current, and EGL gives the functions of its
        // API.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |function| function as *const _)
            })
        };
        egl_context.done_current();
        Ok(HeadlessBackend {
            renderer_backend: RendererBackend::new(mode, gl, Box::new(egl_context)),
            renderable: RenderableBase::new(size, None),
            swap_behaviour,
        })
    }

    pub fn gl_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// `GL_RENDERER`: the name of the GL implementation.
    pub fn get_renderer_name(&self) -> String {
        let _scope = BackendScope::new(self, ScopeType::Explicit);
        // SAFETY: the context is current.
        unsafe { self.renderer_backend.gl().get_parameter_string(glow::RENDERER) }
    }

    fn with_resource<R>(&self, f: impl FnOnce(&HeadlessRenderableResource) -> R) -> Option<R> {
        let resource = self.get_resource()?;
        downcast_resource::<HeadlessRenderableResource>(&*resource).map(f)
    }
}

impl Drop for HeadlessBackend {
    fn drop(&mut self) {
        // The renderable resource goes before the context.
        self.set_resource(None);
        self.renderer_backend.reset_context();
    }
}

impl Renderable for HeadlessBackend {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for HeadlessBackend {
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
            && let Some(resource) = HeadlessRenderableResource::new(context, self.get_size(), self.swap_behaviour)
        {
            self.set_resource(Some(RenderableResourceBox::new_rc(resource)));
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

impl headless_backend::HeadlessBackend for HeadlessBackend {
    fn read_still_image(&self) -> PremultipliedImage {
        let _scope = BackendScope::new(self, ScopeType::Explicit);
        self.with_resource(HeadlessRenderableResource::read_still_image)
            .unwrap_or_else(|| PremultipliedImage::new(self.get_size()))
    }

    fn get_renderer_backend(&self) -> &dyn renderer_backend::RendererBackend {
        self
    }
}
