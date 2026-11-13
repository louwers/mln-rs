// `vulkan::UniformBufferArray` adds a descriptor set to the gfx array and
// marks it dirty when a buffer is set. The port keeps the gfx struct, which
// has no hook for that, next to a `descriptor_set::UniformDescriptorSet`
// that compares the array with what it last wrote
// (`UniformDescriptorSet::bind`).

use ash::vk;
use mln_gfx::context::Context as _;
use mln_gfx::uniform_buffer;
use mln_util::logging::{Event, Log};

use crate::buffer_resource::BufferResource;
use crate::context::Context;

pub struct UniformBuffer {
    buffer: BufferResource,
    /// Whether the buffer binds as a storage buffer: a layer's consolidated
    /// buffer, which its drawables index.
    storage: bool,
}

impl UniformBuffer {
    pub fn new(buffer: BufferResource, storage: bool) -> Self {
        if let Some(context) = buffer.context() {
            let mut stats = context.rendering_stats();
            stats.num_uniform_buffers += 1;
            stats.mem_uniform_buffers += buffer.get_size_in_bytes() as i32;
        }
        UniformBuffer { buffer, storage }
    }

    pub fn create(context: &Context, data: &[u8], persistent: bool, ssbo: bool) -> Self {
        let usage = if ssbo { vk::BufferUsageFlags::STORAGE_BUFFER } else { vk::BufferUsageFlags::UNIFORM_BUFFER };
        UniformBuffer::new(context.create_buffer(Some(data), data.len(), usage, persistent), ssbo)
    }

    pub fn get_buffer_resource(&self) -> &BufferResource {
        &self.buffer
    }

    pub fn is_storage(&self) -> bool {
        self.storage
    }
}

impl Drop for UniformBuffer {
    fn drop(&mut self) {
        if let Some(context) = self.buffer.context() {
            let mut stats = context.rendering_stats();
            stats.num_uniform_buffers -= 1;
            stats.mem_uniform_buffers -= self.buffer.get_size_in_bytes() as i32;
        }
    }
}

impl uniform_buffer::UniformBuffer for UniformBuffer {
    fn update(&self, data: &[u8]) {
        let size = self.buffer.get_size_in_bytes();
        debug_assert!(data.len() <= size);
        if data.len() > size {
            Log::error(
                Event::General,
                format!("Mismatched size given to UBO update, expected max {size}, got {}", data.len()),
            );
            return;
        }
        if let Some(context) = self.buffer.context() {
            let mut stats = context.rendering_stats();
            stats.num_uniform_updates += 1;
            stats.uniform_update_bytes += data.len();
        }
        self.buffer.update(data, 0);
    }

    fn get_size(&self) -> usize {
        self.buffer.get_size_in_bytes()
    }
}
