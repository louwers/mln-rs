// Resources hold an `Rc<Context>` where C++ holds `Context&`. So that the
// context can be freed, it keeps no `Rc` to anything that points back at
// it.

use std::cell::RefMut;
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::renderer_backend::BackendImpl;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
}

impl Context {
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new(Context { base: ContextBase::new(Rc::clone(backend.released())), backend })
    }

    /// Port-only: drops what the context caches, before the backend lets go
    /// of it.
    pub(crate) fn release_resources(&self) {
        *self.mutable_global_uniform_buffers() = UniformBufferArray::new();
        self.release_cached_resources();
        self.destroy_released();
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::Metal
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.destroy_released();
    }

    fn reduce_memory_usage(&self) {}

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu().borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
