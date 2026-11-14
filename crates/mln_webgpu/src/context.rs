use std::cell::RefMut;
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::{Gpu, TextureFormat};

use crate::renderer_backend::BackendImpl;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
}

impl Context {
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new(Context { base: ContextBase::new(Rc::clone(backend.released())), backend })
    }
}

impl GfxContext for Context {
    fn context_base(&self) -> &ContextBase {
        &self.base
    }

    fn backend_type(&self) -> BackendType {
        BackendType::WebGPU
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {}

    fn reduce_memory_usage(&self) {}

    /// Depth only, which every render target's pass clears.
    fn render_target_depth(&self) -> Option<TextureFormat> {
        Some(TextureFormat::Depth32Float)
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu().borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
