// Ports include/mln/gl/renderer_backend.hpp and src/mln/gl/renderer_backend.cpp
//
// The GL functions and the platform's context are shared (`Rc<BackendImpl>`)
// by the backend and its context: resources reach them through the context,
// as C++'s do through `Context& -> getBackend()`.
//
// `getDefaultRenderable()` is implemented by the platform backends
// (HeadlessBackend, SurfaceBackend, CanvasBackend), which embed this struct.
//
// Not ported: `assumeFramebufferBinding`, `assumeViewport`,
// `assumeScissorTest` and the checks that go with them, for hosts that bind
// the framebuffer themselves, and `getExtensionFunctionPointer`: glow loads
// the functions.

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::renderer_backend::{ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::{REGISTERED_SHADERS, shader_source};
use crate::types::Gl;

/// The platform's GL context, which a backend makes current around its
/// frames (C++: the pure virtual `activate` and `deactivate`).
pub trait PlatformContext {
    fn make_current(&self);
    fn done_current(&self);
}

/// The context state the backend and its `Context` share.
pub struct BackendImpl {
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
    pub fn gl(&self) -> &Gl {
        &self.gl
    }

    /// The GL functions, to share with code that draws on the context's
    /// thread.
    pub fn gl_shared(&self) -> &Arc<Gl> {
        &self.gl
    }

    #[cfg(target_family = "wasm")]
    pub fn webgl2(&self) -> &web_sys::WebGl2RenderingContext {
        &self.webgl2
    }

    pub fn make_current(&self) {
        self.platform.make_current();
    }

    pub fn done_current(&self) {
        self.platform.done_current();
    }
}

/// `gl::RendererBackend`: embedded by the platform backends, which implement
/// `gfx::RendererBackend` by delegating to it.
pub struct RendererBackend {
    pub base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
}

impl RendererBackend {
    /// `RendererBackend(contextMode)`, on the functions of `platform`'s
    /// context.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(context_mode: ContextMode, gl: Gl, platform: Box<dyn PlatformContext>) -> Self {
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(gl);
        RendererBackend { base: RendererBackendBase::new(context_mode), impl_: Rc::new(BackendImpl { gl, platform }) }
    }

    /// `RendererBackend(contextMode)`, on a canvas's WebGL2 context.
    #[cfg(target_family = "wasm")]
    pub fn new(
        context_mode: ContextMode,
        webgl2: web_sys::WebGl2RenderingContext,
        platform: Box<dyn PlatformContext>,
    ) -> Self {
        #[allow(clippy::arc_with_non_send_sync)]
        let gl = Arc::new(glow::Context::from_webgl2_context(webgl2.clone()));
        RendererBackend {
            base: RendererBackendBase::new(context_mode),
            impl_: Rc::new(BackendImpl { gl, webgl2, platform }),
        }
    }

    pub fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    pub fn gl(&self) -> &Gl {
        self.impl_.gl()
    }

    /// `context.reset()`, with the context current: its objects are deleted
    /// (`Context::release_resources`).
    pub(crate) fn reset_context(&mut self) {
        if let Some(context) = self.base.reset_context()
            && let Some(context) = context.as_any().downcast_ref::<Context>()
        {
            self.impl_.make_current();
            context.release_resources();
            self.impl_.done_current();
        }
    }

    /// `activate()`
    pub fn activate(&self) {
        self.impl_.make_current();
    }

    /// `deactivate()`
    pub fn deactivate(&self) {
        self.impl_.done_current();
    }

    /// `createContext()`
    pub fn create_context(&self) -> Rc<Context> {
        Context::new(self.impl_.clone())
    }

    /// `initShaders`: register one `ShaderGroupGL` per built-in shader. A
    /// duplicate registration is an error, which C++ throws and the port
    /// logs.
    pub fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
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

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
    }
}
