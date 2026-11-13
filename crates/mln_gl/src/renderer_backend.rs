// The GL functions and the platform's context are shared (`Rc<BackendImpl>`)
// by the backend and its context: resources reach them through the context,
// as C++'s do through `Context& -> getBackend()`.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`, `CanvasTarget`).
//
// Not ported: `assumeFramebufferBinding`, `assumeViewport`,
// `assumeScissorTest` and the checks that go with them, for hosts that bind
// the framebuffer themselves, and `getExtensionFunctionPointer`: glow loads
// the functions.

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::{REGISTERED_SHADERS, shader_source};
use crate::types::Gl;

/// The platform's GL context, which a backend makes current around its
/// frames (C++: the pure virtual `activate` and `deactivate`).
pub(crate) trait PlatformContext {
    fn make_current(&self);
    fn done_current(&self);

    /// Whether something else used the context since the backend's last
    /// `make_current`, so that the GL state its context cached is gone.
    /// Reading it clears it.
    fn take_state_lost(&self) -> bool {
        false
    }
}

/// The context state the backend and its `Context` share.
pub(crate) struct BackendImpl {
    // Dropped before the platform's context, which its functions belong to.
    // An `Arc` for the code that draws beside the backend with a library
    // that wants one (a window's overlay); it stays on the context's thread.
    gl: Arc<Gl>,
    /// The context `gl` calls, for the calls that take a JS array.
    #[cfg(target_family = "wasm")]
    webgl2: web_sys::WebGl2RenderingContext,
    platform: Box<dyn PlatformContext>,
}

impl BackendImpl {
    #[must_use]
    pub(crate) fn gl(&self) -> &Gl {
        &self.gl
    }

    /// The GL functions, to share with code that draws on the context's
    /// thread.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[must_use]
    pub(crate) fn gl_shared(&self) -> &Arc<Gl> {
        &self.gl
    }

    #[cfg(target_family = "wasm")]
    #[must_use]
    pub(crate) fn webgl2(&self) -> &web_sys::WebGl2RenderingContext {
        &self.webgl2
    }

    pub(crate) fn make_current(&self) {
        self.platform.make_current();
    }

    /// Whether something else used the context since the last
    /// `make_current`; reading it clears it.
    #[must_use]
    pub(crate) fn take_state_lost(&self) -> bool {
        self.platform.take_state_lost()
    }

    pub(crate) fn done_current(&self) {
        self.platform.done_current();
    }
}

/// A renderer's GL context.
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
}

impl RendererBackend {
    /// A backend on a context of `device`'s share group, which the backend
    /// makes current around its frames.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when EGL makes
    /// no context, and with [`ErrorKind::Unsupported`](mln_util::ErrorKind)
    /// on the host's device.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(device: &crate::device::Device) -> Result<Self, mln_util::error::Error> {
        let (gl, platform) = device.create_context()?;
        Ok(Self::from_parts(gl, platform))
    }

    /// A backend on the functions of `platform`'s context.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn from_parts(gl: Gl, platform: Box<dyn PlatformContext>) -> Self {
        let gl = Arc::new(gl);
        RendererBackend { base: RendererBackendBase::new(), impl_: Rc::new(BackendImpl { gl, platform }) }
    }

    /// A backend on a canvas's WebGL2 context.
    #[cfg(target_family = "wasm")]
    pub(crate) fn from_parts(webgl2: web_sys::WebGl2RenderingContext, platform: Box<dyn PlatformContext>) -> Self {
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "the backend lives on the browser's one thread, and the `Arc` only shares the WebGL2 context among its objects"
        )]
        let gl = Arc::new(glow::Context::from_webgl2_context(webgl2.clone()));
        RendererBackend { base: RendererBackendBase::new(), impl_: Rc::new(BackendImpl { gl, webgl2, platform }) }
    }

    /// The backend's context, made on first use. Call inside a
    /// `BackendScope`.
    pub(crate) fn gl_context(&self) -> Option<Rc<Context>> {
        let context = renderer_backend::RendererBackend::get_context(self);
        context.downcast_ref::<Context>().map(Context::rc)
    }

    /// `GL_RENDERER`: the name of the GL implementation. Call inside a
    /// `BackendScope`.
    pub fn get_renderer_name(&self) -> String {
        use glow::HasContext as _;
        // SAFETY: the context is current.
        unsafe { self.gl().get_parameter_string(glow::RENDERER) }
    }

    pub(crate) fn gl(&self) -> &Gl {
        self.impl_.gl()
    }

    /// Drops the context, deleting its objects with the context current
    /// (`Context::release_resources`).
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context()
            && let Some(context) = context.downcast_ref::<Context>()
        {
            self.impl_.make_current();
            context.release_resources();
            self.impl_.done_current();
        }
    }

    /// Makes the backend's context current. Where another renderer used the
    /// context since the backend's last frame, the context forgets the GL state
    /// it cached.
    pub(crate) fn activate(&self) {
        self.impl_.make_current();
        if self.impl_.take_state_lost()
            && let Some(context) = self.base.existing_context()
            && let Some(context) = context.downcast_ref::<Context>()
        {
            context.set_dirty_state();
        }
    }

    pub(crate) fn deactivate(&self) {
        self.impl_.done_current();
    }

    pub(crate) fn create_context(&self) -> Rc<Context> {
        Context::new(Rc::clone(&self.impl_))
    }

    /// Registers one shader group per built-in shader. A duplicate
    /// registration is an error, which C++ throws and this logs.
    #[expect(clippy::unused_self, reason = "each backend registers its shaders through its own `init_shaders`")]
    pub(crate) fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
        for &shader_id in REGISTERED_SHADERS {
            let Some(source) = shader_source(shader_id) else {
                continue;
            };
            let group = Rc::new(ShaderGroup::new(shader_id, program_parameters.clone()));
            if !registry.register_shader_group(group, source.name) {
                Log::error(Event::Shader, format!("Failed to register {} with shader registry!", source.name));
                debug_assert!(false, "duplicate shader group");
            }
        }
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        RendererBackend::init_shaders(self, registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        RendererBackend::create_context(self)
    }

    fn activate(&self) {
        RendererBackend::activate(self);
    }

    fn deactivate(&self) {
        RendererBackend::deactivate(self);
    }
}

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
    }
}
