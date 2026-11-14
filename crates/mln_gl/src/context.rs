// Resources hold a `Weak<Context>` where C++ holds `Context&`: a resource
// that outlives the context has nothing left to delete, and the context can
// cache resources of its own.
//
// Not ported: the buffer allocator and the texture pool (each buffer and
// texture is a GL object of its own), fences, the legacy program path's
// renderbuffers, draw scopes and vertex array state, and the debugging and
// timestamp query extensions.

use std::cell::{RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use glow::HasContext;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::logging::{Event, Log};

use crate::gpu::GlGpu;
use crate::renderer_backend::BackendImpl;
use crate::state::State;
use crate::types::{FramebufferID, Gl, TextureID};

/// The objects whose owners are gone, deleted by `end_frame` while the
/// context is current.
#[derive(Default)]
struct Abandoned {
    textures: Vec<TextureID>,
    framebuffers: Vec<FramebufferID>,
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
        })
    }

    /// The owning `Rc`, for the passes of a frame.
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("gl::Context used after drop")
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
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

    /// The GL state the context last set.
    #[cfg(not(target_arch = "wasm32"))]
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

    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn abandon_framebuffer(&self, framebuffer: FramebufferID) {
        self.abandoned.borrow_mut().framebuffers.push(framebuffer);
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
        }
    }

    // --- render state ---

    /// Forgets the GL state the context tracks, so that the next draw sets
    /// all of it again. Call it after code outside the context changed it.
    pub(crate) fn set_dirty_state(&self) {
        self.state.set_dirty();
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

    /// Deletes the objects abandoned since the last frame.
    fn end_frame(&self) {
        // What draws on the context outside the frames samples with its
        // textures' own parameters.
        self.state.unbind_samplers(self.gl());
        self.log_errors();
        self.delete_abandoned();
    }

    fn reduce_memory_usage(&self) {
        self.delete_abandoned();
        self.destroy_released();
        // Ensure that all pending actions are executed to ensure that they
        // happen before the app goes to the background.
        // SAFETY: no preconditions.
        unsafe { self.gl().finish() };
    }

    /// Unsigned bytes where a half-float texture can't be rendered into (a
    /// WebGL2 without `EXT_color_buffer_float`).
    fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        if type_ == TextureChannelDataType::UnsignedByte || self.half_float_renderable {
            type_
        } else {
            TextureChannelDataType::UnsignedByte
        }
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
