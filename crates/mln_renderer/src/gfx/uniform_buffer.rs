use mln_gfx::buffer::BufferPtr;
use mln_gfx::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;

use crate::gfx::context::Context;

/// A buffer of [`Uniform`](mln_gfx::gpu::BufferUsage::Uniform) or
/// [`Storage`](mln_gfx::gpu::BufferUsage::Storage) usage.
pub(crate) type UniformBufferPtr = BufferPtr;

/// A collection of uniform buffers by id.
#[derive(Clone, Default)]
pub(crate) struct UniformBufferArray {
    uniform_buffer_vector: [Option<UniformBufferPtr>; MAX_UBO_COUNT_PER_SHADER],
}

impl UniformBufferArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of maximum allocated elements
    #[must_use]
    pub(crate) fn allocated_size(&self) -> usize {
        self.uniform_buffer_vector.len()
    }

    /// Get an uniform buffer element, or `None` if it doesn't exist.
    pub(crate) fn get(&self, id: usize) -> Option<&UniformBufferPtr> {
        self.uniform_buffer_vector.get(id).and_then(Option::as_ref)
    }

    /// Set a new uniform buffer element or replace the existing one.
    pub(crate) fn set(&mut self, id: usize, uniform_buffer: Option<UniformBufferPtr>) -> Option<&UniformBufferPtr> {
        debug_assert!(id < self.uniform_buffer_vector.len());
        if id >= self.uniform_buffer_vector.len() {
            return None;
        }
        self.uniform_buffer_vector[id] = uniform_buffer;
        self.uniform_buffer_vector[id].as_ref()
    }

    /// Writes `data` to the uniform buffer `id`, first making one if there
    /// is none or `data` doesn't fit in it.
    pub(crate) fn create_or_update_pod<T: bytemuck::Pod>(&mut self, id: usize, data: &T, context: &Context) {
        context.write_uniform_buffer(&mut self.uniform_buffer_vector[id], bytemuck::bytes_of(data));
    }
}
