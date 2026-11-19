// What a GL backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`, `CanvasTarget`, `BorrowedTarget`, `ExportedTarget`).

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use mln_gfx::renderer_backend;

use crate::gpu::GlGpu;
use crate::state::State;
use crate::types::Gl;

/// The platform's GL context, which [`GlContext`] makes current around the
/// calls that use it.
pub(crate) trait PlatformContext {
    fn make_current(&mut self);
    fn done_current(&mut self);

    /// Whether something else used the context since the last
    /// `make_current`, so that the GL state cached for it is gone. Reading
    /// it clears it.
    fn take_state_lost(&mut self) -> bool {
        false
    }

    /// The window that the context presents to, for a window's context.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    fn window(&mut self) -> Option<&mut crate::surface_target::EglWindow> {
        None
    }
}

/// A GL context: the platform's, the functions that call it and the GL
/// state last set on it. Each call that uses it makes it current for the
/// call with [`make_current`](Self::make_current); calls inside it keep it
/// current.
pub(crate) struct GlContext {
    // Dropped before the platform's context, which its functions belong to.
    // An `Arc` for the code that draws beside the backend with a library
    // that wants one (a window's overlay); it stays on the context's thread.
    gl: Arc<Gl>,
    /// The context `gl` calls, for the calls that take a JS array.
    #[cfg(target_family = "wasm")]
    webgl2: web_sys::WebGl2RenderingContext,
    state: State,
    platform: Box<dyn PlatformContext>,
    /// How many calls have the context current.
    depth: u32,
}

impl GlContext {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn new(gl: Gl, platform: Box<dyn PlatformContext>) -> GlContext {
        GlContext { gl: Arc::new(gl), state: State::default(), platform, depth: 0 }
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn new(webgl2: web_sys::WebGl2RenderingContext, platform: Box<dyn PlatformContext>) -> GlContext {
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "the backend lives on the browser's one thread, and the `Arc` only shares the WebGL2 context among its objects"
        )]
        let gl = Arc::new(glow::Context::from_webgl2_context(webgl2.clone()));
        GlContext { gl, webgl2, state: State::default(), platform, depth: 0 }
    }

    /// The GL functions, which call the context while it is current.
    pub(crate) fn gl(&self) -> &Gl {
        &self.gl
    }

    /// The GL functions, to share with code that draws on the context's
    /// thread.
    pub(crate) fn gl_shared(&self) -> &Arc<Gl> {
        &self.gl
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn webgl2(&self) -> &web_sys::WebGl2RenderingContext {
        &self.webgl2
    }

    /// The GL functions, and the GL state last set on the context, which
    /// calls through them set.
    pub(crate) fn parts(&mut self) -> (&Gl, &mut State) {
        (&self.gl, &mut self.state)
    }

    /// The platform's context.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn platform(&mut self) -> &mut dyn PlatformContext {
        &mut *self.platform
    }

    /// Makes the context current, unless a call outside this one has it
    /// current already. Where something else used the context since, the
    /// cached GL state is forgotten. Each call is ended by a
    /// [`done_current`](Self::done_current).
    pub(crate) fn make_current(&mut self) {
        self.depth += 1;
        if self.depth == 1 {
            self.platform.make_current();
            if self.platform.take_state_lost() {
                self.state.set_dirty();
            }
        }
    }

    /// Ends a [`make_current`](Self::make_current): the outermost one lets
    /// go of the context.
    pub(crate) fn done_current(&mut self) {
        self.depth -= 1;
        if self.depth == 0 {
            self.platform.done_current();
        }
    }
}

/// A renderer's [`GlGpu`], which owns its GL context.
pub struct RendererBackend {
    /// The `GlGpu` that owns the renderer's resources.
    gpu: GlGpu,
}

impl RendererBackend {
    /// A backend on a context of `device`'s share group.
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
        RendererBackend { gpu: GlGpu::on(GlContext::new(gl, platform)) }
    }

    /// A backend on a canvas's WebGL2 context.
    #[cfg(target_family = "wasm")]
    pub(crate) fn from_parts(webgl2: web_sys::WebGl2RenderingContext, platform: Box<dyn PlatformContext>) -> Self {
        RendererBackend { gpu: GlGpu::on(GlContext::new(webgl2, platform)) }
    }

    /// `GL_RENDERER`: the name of the GL implementation.
    #[must_use]
    pub fn get_renderer_name(&self) -> String {
        self.gpu.renderer_name().to_owned()
    }

    /// The backend's context.
    #[cfg(test)]
    pub(crate) fn context(&self) -> &GlContext {
        self.gpu.context()
    }
}

/// The backend's [`GlGpu`], lent with its context current: the calls in
/// between don't each make it current and release it, which flushes on
/// Mesa. The scope's end leaves no samplers bound, for what draws on the
/// context outside the renderer, and in debug builds logs an error GL
/// recorded; a scope that a panic unwinds through only releases the
/// context.
pub struct GlScope<'a> {
    gpu: &'a mut GlGpu,
}

impl Deref for GlScope<'_> {
    type Target = GlGpu;

    fn deref(&self) -> &GlGpu {
        self.gpu
    }
}

impl DerefMut for GlScope<'_> {
    fn deref_mut(&mut self) -> &mut GlGpu {
        self.gpu
    }
}

impl Drop for GlScope<'_> {
    fn drop(&mut self) {
        // No GL calls while a panic unwinds, which a panic in them would
        // turn into an abort.
        if !std::thread::panicking() {
            self.gpu.end_frame();
        }
        self.gpu.context_mut().done_current();
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = GlGpu;
    type Scope<'a> = GlScope<'a>;

    /// Lends the `GlGpu` with the context current until the scope drops.
    fn gpu(&mut self) -> GlScope<'_> {
        self.gpu.context_mut().make_current();
        GlScope { gpu: &mut self.gpu }
    }
}
