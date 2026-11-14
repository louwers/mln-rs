// Resources hold `Rc<Context>`, so the context keeps no `Rc` to a resource
// that points back at it; otherwise the context, and with it the device,
// would never be freed.

use std::cell::{Cell, RefMut};
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::types::TextureChannelDataType;
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::offscreen_texture::OffscreenTexture;
use crate::renderer_backend::BackendImpl;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
    /// Command buffers the command encoders have submitted.
    submissions: Cell<u64>,
}

impl Context {
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(Rc::clone(backend.released())),
            backend,
            weak_self: Weak::clone(weak),
            submissions: Cell::new(0),
        })
    }

    /// How many command buffers the command encoders have submitted,
    /// counting one as it is being submitted.
    pub(crate) fn submissions(&self) -> u64 {
        self.submissions.get()
    }

    pub(crate) fn count_submission(&self) {
        self.submissions.set(self.submissions.get() + 1);
    }

    /// The owning `Rc`, which resources keep to keep the context alive.
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("webgpu::Context used after drop")
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    pub(crate) fn rendering_stats_mut(&self) -> RefMut<'_, RenderingStats> {
        GfxContext::rendering_stats(self)
    }

    pub(crate) fn create_offscreen_texture_with(
        &self,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> OffscreenTexture {
        OffscreenTexture::new(self.rc(), size, type_, depth, stencil)
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

    fn end_frame(&self) {
        self.destroy_released();
    }

    fn perform_cleanup(&self) {
        let mut stats = self.rendering_stats_mut();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {}

    /// Creates an offscreen texture of `size` with `type_` channels, with
    /// depth but no stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(self.create_offscreen_texture_with(size, type_, true, false))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu().borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
