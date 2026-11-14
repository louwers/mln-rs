// Resources hold a `Weak<Context>` where C++ holds `Context&`: a resource
// that outlives the context has nothing left to delete, and the context can
// cache resources of its own.
//
// Not ported: the buffer allocator and the texture pool (each buffer and
// texture is a GL object of its own), fences, the legacy program path's
// renderbuffers, draw scopes and vertex array state, and the debugging and
// timestamp query extensions.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use glow::HasContext;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::{Gpu, TextureRegion};
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::gpu::GlGpu;
use crate::offscreen_texture::OffscreenTexture;
use crate::renderer_backend::BackendImpl;
use crate::state::State;
use crate::types::{FramebufferID, Gl, RenderbufferID, TextureID};

/// The objects whose owners are gone, deleted by `perform_cleanup` while the
/// context is current.
#[derive(Default)]
struct Abandoned {
    textures: Vec<TextureID>,
    framebuffers: Vec<FramebufferID>,
    renderbuffers: Vec<RenderbufferID>,
}

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    state: Rc<State>,
    /// The `Gpu` the renderer's buffers belong to, on this context and
    /// sharing its GL state.
    gpu: RefCell<GlGpu>,
    /// Whether a half-float texture can be rendered into
    /// (`EXT_color_buffer_half_float` or `EXT_color_buffer_float`).
    half_float_renderable: bool,
    abandoned: RefCell<Abandoned>,
    /// The framebuffer being drawn into, none for the default one.
    framebuffer: Cell<Option<FramebufferID>>,
    /// The size of the framebuffer being drawn into.
    framebuffer_size: Cell<Size>,
    /// Whether that framebuffer has a stencil buffer.
    framebuffer_has_stencil: Cell<bool>,
}

impl Context {
    /// Creates the context of `backend`, whose GL context must be current.
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        let gl = backend.gl();
        let extensions = gl.supported_extensions();
        let desktop = !gl.version().is_embedded;
        // Desktop OpenGL 3.0 renders into half-float textures without an
        // extension.
        let half_float_renderable = desktop
            || extensions.contains("EXT_color_buffer_half_float")
            || extensions.contains("GL_EXT_color_buffer_half_float")
            || extensions.contains("EXT_color_buffer_float")
            || extensions.contains("GL_EXT_color_buffer_float");
        let state = Rc::new(State::default());
        // On desktop OpenGL, this turns on the sRGB encoding of what is
        // written to an sRGB framebuffer, such as the host's texture.
        #[cfg(target_family = "wasm")]
        let gpu = GlGpu::hosted(Arc::clone(backend.gl_shared()), Rc::clone(&state), backend.webgl2().clone());
        #[cfg(not(target_family = "wasm"))]
        let gpu = GlGpu::hosted(Arc::clone(backend.gl_shared()), Rc::clone(&state));
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::default(),
            backend,
            weak_self: Weak::clone(weak),
            state,
            gpu: RefCell::new(gpu),
            half_float_renderable,
            abandoned: RefCell::new(Abandoned::default()),
            framebuffer: Cell::new(None),
            framebuffer_size: Cell::new(Size::new(0, 0)),
            framebuffer_has_stencil: Cell::new(false),
        })
    }

    /// The owning `Rc`, for the passes of a frame.
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("gl::Context used after drop")
    }

    pub(crate) fn weak(&self) -> Weak<Context> {
        Weak::clone(&self.weak_self)
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// The GL functions of the backend's context.
    pub(crate) fn gl(&self) -> &Gl {
        self.backend.gl()
    }

    /// The GL functions, to share with a custom layer.
    pub(crate) fn gl_shared(&self) -> &Arc<Gl> {
        self.backend.gl_shared()
    }

    /// The GL state the context last set.
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    /// Port-only: deletes what the context caches and what is abandoned,
    /// before the backend lets go of it. The context must be current.
    pub(crate) fn release_resources(&self) {
        *self.mutable_global_uniform_buffers() = UniformBufferArray::new();
        self.release_cached_resources();
        self.delete_abandoned();
        self.destroy_released();
        self.gpu.borrow_mut().release();
    }

    // --- objects ---

    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn abandon_texture(&self, texture: TextureID) {
        self.abandoned.borrow_mut().textures.push(texture);
    }

    pub(crate) fn abandon_framebuffer(&self, framebuffer: FramebufferID) {
        self.abandoned.borrow_mut().framebuffers.push(framebuffer);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn abandon_renderbuffer(&self, renderbuffer: RenderbufferID) {
        self.abandoned.borrow_mut().renderbuffers.push(renderbuffer);
    }

    /// `MBGL_CHECK_ERROR`, once a frame and in debug builds only, as in
    /// C++: logs an error GL has recorded. Asking GL waits for it to run
    /// every call so far.
    fn log_errors(&self) {
        if !cfg!(debug_assertions) {
            return;
        }
        // SAFETY: no preconditions.
        let error = unsafe { self.gl().get_error() };
        if error != glow::NO_ERROR {
            Log::error(Event::GraphicsBackend, format!("OpenGL error {error:#x} in this frame"));
        }
    }

    /// Deletes the abandoned objects. The context must be current.
    fn delete_abandoned(&self) {
        let abandoned = std::mem::take(&mut *self.abandoned.borrow_mut());
        let gl = self.gl();
        // SAFETY: each object was created by this context and has no owner
        // left; the state forgets the ones it has bound.
        unsafe {
            for texture in abandoned.textures {
                self.state.forget_texture(texture);
                gl.delete_texture(texture);
            }
            for framebuffer in abandoned.framebuffers {
                self.state.forget_framebuffer(framebuffer);
                gl.delete_framebuffer(framebuffer);
            }
            for renderbuffer in abandoned.renderbuffers {
                gl.delete_renderbuffer(renderbuffer);
            }
        }
    }

    // --- render state ---

    /// Forgets the GL state the context tracks, so that the next draw sets
    /// all of it again. Call it after code outside the context changed it.
    pub(crate) fn set_dirty_state(&self) {
        self.state.set_dirty();
    }

    pub(crate) fn clear(&self, color: Option<Color>, depth: Option<f32>, stencil: Option<i32>) {
        self.state.clear(self.gl(), color, depth, stencil);
    }

    /// Binds `framebuffer` (none: the default one) to draw all of its `size`
    /// (C++: the `bind` of each renderable resource).
    pub(crate) fn bind_framebuffer(&self, framebuffer: Option<FramebufferID>, size: Size, has_stencil: bool) {
        let gl = self.gl();
        self.state.bind_framebuffer(gl, framebuffer);
        self.state.set_scissor(gl, None);
        self.state.set_viewport(gl, (0, 0, size.width as i32, size.height as i32));
        self.framebuffer.set(framebuffer);
        self.framebuffer_size.set(size);
        self.framebuffer_has_stencil.set(has_stencil);
    }

    /// The framebuffer being drawn into, as `bind_framebuffer` bound it: none
    /// for the default one, its size, and whether it has a stencil buffer.
    pub(crate) fn bound_framebuffer(&self) -> (Option<FramebufferID>, Size, bool) {
        (self.framebuffer.get(), self.framebuffer_size.get(), self.framebuffer_has_stencil.get())
    }

    /// Reads the RGBA pixels of the bound framebuffer, of `size`, the top
    /// row first.
    pub(crate) fn read_framebuffer(&self, size: Size) -> Vec<u8> {
        crate::gpu::read_pixels(self.gl(), &self.state, size.height, TextureRegion::whole(size))
    }

    /// The `Gpu` the renderer's resources belong to and its draws encode
    /// through.
    pub(crate) fn gl_gpu(&self) -> Ref<'_, GlGpu> {
        self.gpu.borrow()
    }

    pub(crate) fn gl_gpu_mut(&self) -> RefMut<'_, GlGpu> {
        self.gpu.borrow_mut()
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::OpenGL
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        // What draws on the context outside the frames samples with its
        // textures' own parameters.
        self.state.unbind_samplers(self.gl());
        self.log_errors();
    }

    fn finish(&self) {
        // SAFETY: glFinish takes no arguments.
        unsafe { self.gl().finish() };
    }

    /// Called at the end of a frame, when its command encoder goes: deletes
    /// the objects abandoned since the last one.
    fn perform_cleanup(&self) {
        self.delete_abandoned();
        self.destroy_released();
    }

    fn reduce_memory_usage(&self) {
        self.delete_abandoned();
        self.destroy_released();
        // Ensure that all pending actions are executed to ensure that they
        // happen before the app goes to the background.
        // SAFETY: no preconditions.
        unsafe { self.gl().finish() };
    }

    /// Creates an offscreen texture of `size` with `type_` channels. Where a
    /// half-float texture can't be rendered into (a WebGL2 without
    /// `EXT_color_buffer_float`), the texture has bytes, as the heatmap's has
    /// in C++ on such a context.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        let type_ = if type_ == TextureChannelDataType::UnsignedByte || self.half_float_renderable {
            type_
        } else {
            TextureChannelDataType::UnsignedByte
        };
        Box::new(OffscreenTexture::new(self, size, type_))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
