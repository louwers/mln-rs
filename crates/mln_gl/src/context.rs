use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::released::Released;
use mln_gfx::types::TextureChannelDataType;

use crate::gpu::GlGpu;

/// What the backend and its `Context` share.
pub(crate) struct BackendImpl {
    gpu: RefCell<GlGpu>,
    released: Rc<Released>,
}

impl BackendImpl {
    pub(crate) fn new(gpu: GlGpu) -> BackendImpl {
        BackendImpl { gpu: RefCell::new(gpu), released: Rc::default() }
    }

    pub(crate) fn gpu(&self) -> RefMut<'_, GlGpu> {
        self.gpu.borrow_mut()
    }
}

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
}

impl Context {
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new(Context { base: ContextBase::new(Rc::clone(&backend.released)), backend })
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
        self.backend.gpu().end_frame();
    }

    fn reduce_memory_usage(&self) {
        self.destroy_released();
        // What the GPU still runs finishes before the app goes to the
        // background.
        let _ = self.backend.gpu().finish();
    }

    /// Unsigned bytes where a half-float texture can't be rendered into (a
    /// WebGL2 without `EXT_color_buffer_float`).
    fn render_target_type(&self, type_: TextureChannelDataType) -> TextureChannelDataType {
        if type_ == TextureChannelDataType::UnsignedByte || self.backend.gpu().half_float_renderable() {
            type_
        } else {
            TextureChannelDataType::UnsignedByte
        }
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
