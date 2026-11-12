// Ports include/mln/gfx/renderer_backend.hpp and src/mln/gfx/renderer_backend.cpp
//
// TODO(port): `TaggedScheduler threadPool` / `getThreadPool()` (render jobs
// scheduled by background workers) arrives with the first user; the WebGPU
// context calls `runRenderJobs()` in begin/endFrame, a no-op until then.

use std::any::Any;
use std::cell::OnceCell;
use std::rc::Rc;

use mln_util::identity::SimpleIdentity;

use crate::backend_scope::BackendScope;
use crate::context::Context;
use crate::shader_registry::ShaderRegistry;
use crate::shaders::program_parameters::ProgramParameters;

/// The data members of `gfx::RendererBackend`.
pub struct RendererBackendBase {
    pub unique_id: SimpleIdentity,
    context: OnceCell<Rc<dyn Context>>,
}

impl Default for RendererBackendBase {
    fn default() -> Self {
        Self::new()
    }
}

impl RendererBackendBase {
    #[must_use]
    pub fn new() -> Self {
        RendererBackendBase { unique_id: SimpleIdentity::new(), context: OnceCell::new() }
    }

    /// `context.reset()` in the backend destructors. Returns the context, so
    /// a backend can release what it caches before dropping it.
    pub fn reset_context(&mut self) -> Option<Rc<dyn Context>> {
        self.context.take()
    }

    /// Port-only: the context, if the backend has made it.
    #[must_use]
    pub fn existing_context(&self) -> Option<&Rc<dyn Context>> {
        self.context.get()
    }
}

/// A renderer's device-side state: its context and shaders. What it draws
/// into is a `FrameTarget` built on it.
pub trait RendererBackend: Any {
    fn renderer_backend_base(&self) -> &RendererBackendBase;
    fn as_any(&self) -> &dyn Any;

    /// Returns the device's context, created on first use (`std::call_once`).
    /// Must be called inside a `BackendScope`.
    fn get_context(&self) -> &dyn Context {
        &**self.get_context_ptr()
    }

    /// The context as a shared pointer (the C++ context is owned by the
    /// backend; passes and resources in the port hold `Rc`s instead of
    /// references).
    fn get_context_ptr(&self) -> &Rc<dyn Context> {
        debug_assert!(BackendScope::exists());
        self.renderer_backend_base().context.get_or_init(|| self.create_context())
    }

    fn unique_id(&self) -> SimpleIdentity {
        self.renderer_backend_base().unique_id
    }

    /// One-time shader initialization
    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters);

    /// Called when the backend's GL context needs to be made active or
    /// inactive (protected + `friend class BackendScope` in C++).
    fn create_context(&self) -> Rc<dyn Context>;
    fn activate(&self);
    fn deactivate(&self);
}
