// What a GL backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`, `CanvasTarget`, `BorrowedTarget`, `ExportedTarget`).

use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::renderer_backend;

use crate::gpu::GlGpu;
use crate::state::State;
use crate::types::Gl;

/// The platform's GL context, which [`GlContext`] makes current around the
/// calls that use it.
pub(crate) trait PlatformContext {
    fn make_current(&self);
    fn done_current(&self);

    /// Whether something else used the context since the last
    /// `make_current`, so that the GL state cached for it is gone. Reading
    /// it clears it.
    fn take_state_lost(&self) -> bool {
        false
    }
}

/// A GL context: the platform's, the functions that call it and the GL
/// state last set on it. Each call that uses it makes it current for the
/// call with [`enter`](Self::enter); calls inside it keep it current.
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
    depth: Cell<u32>,
}

/// Has the context current until it is dropped.
pub(crate) struct Current(Rc<GlContext>);

impl Drop for Current {
    fn drop(&mut self) {
        self.0.done_current();
    }
}

impl GlContext {
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn new(gl: Gl, platform: Box<dyn PlatformContext>) -> Rc<GlContext> {
        let gl = Arc::new(gl);
        Rc::new(GlContext { gl, state: State::default(), platform, depth: Cell::new(0) })
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn new(webgl2: web_sys::WebGl2RenderingContext, platform: Box<dyn PlatformContext>) -> Rc<GlContext> {
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "the backend lives on the browser's one thread, and the `Arc` only shares the WebGL2 context among its objects"
        )]
        let gl = Arc::new(glow::Context::from_webgl2_context(webgl2.clone()));
        Rc::new(GlContext { gl, webgl2, state: State::default(), platform, depth: Cell::new(0) })
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

    /// The GL state last set on the context.
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// Makes the context current until the returned guard drops.
    pub(crate) fn enter(self: &Rc<Self>) -> Current {
        self.make_current();
        Current(Rc::clone(self))
    }

    /// Makes the context current, unless a call outside this one has it
    /// current already. Where something else used the context since, the
    /// cached GL state is forgotten.
    fn make_current(&self) {
        let depth = self.depth.get();
        self.depth.set(depth + 1);
        if depth == 0 {
            self.platform.make_current();
            if self.platform.take_state_lost() {
                self.state.set_dirty();
            }
        }
    }

    /// Ends a [`make_current`](Self::make_current): the outermost one lets
    /// go of the context.
    fn done_current(&self) {
        let depth = self.depth.get() - 1;
        self.depth.set(depth);
        if depth == 0 {
            self.platform.done_current();
        }
    }

    /// Makes the context current again, where a call has it current, for a
    /// platform's context that is current on another surface now.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn make_current_again(&self) {
        if self.depth.get() > 0 {
            self.platform.make_current();
        }
    }
}

/// A renderer's GL context, and the [`GlGpu`] on it.
pub struct RendererBackend {
    /// The `GlGpu` that owns the renderer's resources.
    gpu: GlGpu,
    context: Rc<GlContext>,
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
        Self::on(GlContext::new(gl, platform))
    }

    /// A backend on a canvas's WebGL2 context.
    #[cfg(target_family = "wasm")]
    pub(crate) fn from_parts(webgl2: web_sys::WebGl2RenderingContext, platform: Box<dyn PlatformContext>) -> Self {
        Self::on(GlContext::new(webgl2, platform))
    }

    fn on(context: Rc<GlContext>) -> Self {
        RendererBackend { gpu: GlGpu::on(Rc::clone(&context)), context }
    }

    /// `GL_RENDERER`: the name of the GL implementation.
    #[must_use]
    pub fn get_renderer_name(&self) -> String {
        use glow::HasContext as _;
        let _current = self.context.enter();
        // SAFETY: the context is current.
        unsafe { self.context.gl().get_parameter_string(glow::RENDERER) }
    }

    /// The backend's context.
    #[cfg(any(test, windows, all(unix, not(target_vendor = "apple"))))]
    pub(crate) fn context(&self) -> &Rc<GlContext> {
        &self.context
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
    current: Option<Current>,
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
        drop(self.current.take());
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = GlGpu;
    type Scope<'a> = GlScope<'a>;

    /// Lends the `GlGpu` with the context current until the scope drops.
    fn gpu(&mut self) -> GlScope<'_> {
        let current = self.context.enter();
        GlScope { gpu: &mut self.gpu, current: Some(current) }
    }
}
