//! The renderer's `gfx::Context` on Vulkan: the state the renderer keeps
//! there, and the backend's [`VulkanGpu`], which owns the renderer's
//! resources and submits its frames.

use std::cell::{RefCell, RefMut};

use mln_gfx::backend::Type as BackendType;
use mln_gfx::context::{Context as GfxContext, ContextBase};
use mln_gfx::gpu::Gpu;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::gpu::VulkanGpu;

pub(crate) struct Context {
    base: ContextBase,
    gpu: RefCell<VulkanGpu>,
}

impl Context {
    pub(crate) fn new(gpu: VulkanGpu) -> Context {
        Context { base: ContextBase::default(), gpu: RefCell::new(gpu) }
    }

    pub(crate) fn vulkan_gpu_mut(&self) -> RefMut<'_, VulkanGpu> {
        self.gpu.borrow_mut()
    }
}

impl Drop for Context {
    /// Destroys the buffers the context keeps for draws, and what the
    /// renderer released, before the `Gpu` goes.
    fn drop(&mut self) {
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
        BackendType::Vulkan
    }

    fn begin_frame(&self) {}
    fn end_frame(&self) {}
    fn reduce_memory_usage(&self) {}

    fn gpu(&self) -> RefMut<'_, dyn Gpu> {
        RefMut::map(self.gpu.borrow_mut(), |gpu| -> &mut dyn Gpu { gpu })
    }
}
