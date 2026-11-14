// Resources hold an `Rc<Context>` where C++ holds `Context&`. So that the
// context can be freed, it keeps no `Rc` to anything that points back at
// it.

use std::cell::RefMut;
use std::rc::{Rc, Weak};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::types::TextureChannelDataType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::offscreen_texture::OffscreenTexture;
use crate::renderer_backend::BackendImpl;

pub(crate) struct Context {
    base: ContextBase,
    backend: Rc<BackendImpl>,
    weak_self: Weak<Context>,
}

impl Context {
    #[must_use]
    pub(crate) fn new(backend: Rc<BackendImpl>) -> Rc<Context> {
        Rc::new_cyclic(|weak| Context {
            base: ContextBase::new(Rc::clone(backend.released())),
            backend,
            weak_self: Weak::clone(weak),
        })
    }

    /// The owning `Rc` (resources keep the context alive, as the C++
    /// `Context&` members assume).
    pub(crate) fn rc(&self) -> Rc<Context> {
        self.weak_self.upgrade().expect("mtl::Context used after drop")
    }

    pub(crate) fn get_backend(&self) -> &Rc<BackendImpl> {
        &self.backend
    }

    /// Port-only: drops what the context caches, before the backend lets go
    /// of it.
    pub(crate) fn release_resources(&self) {
        *self.mutable_global_uniform_buffers() = UniformBufferArray::new();
        self.release_cached_resources();
        self.destroy_released();
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
        BackendType::Metal
    }

    fn begin_frame(&self) {}

    fn end_frame(&self) {
        self.destroy_released();
    }

    /// Called at the end of a frame.
    fn perform_cleanup(&self) {
        let mut stats = self.rendering_stats();
        stats.num_draw_calls = 0;
        stats.num_frames += 1;
    }

    fn reduce_memory_usage(&self) {}

    /// Creates an offscreen texture of `size` with `type_` channels, with
    /// neither depth nor stencil.
    fn create_offscreen_texture(
        &self,
        size: Size,
        type_: TextureChannelDataType,
    ) -> Box<dyn mln_gfx::offscreen_texture::OffscreenTexture> {
        Box::new(self.create_offscreen_texture_with(size, type_, false, false))
    }

    fn create_command_encoder(&self) -> Box<dyn mln_gfx::command_encoder::CommandEncoder> {
        Box::new(CommandEncoder::new(self.rc()))
    }

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.backend.gpu().borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
