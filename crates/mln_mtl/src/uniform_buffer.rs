// `mtl::UniformBufferArray` only adds `bindMtl` to the gfx array, so the
// port keeps the gfx struct and binds it with `bind_uniform_buffer_array`.

use mln_gfx::context::Context as _;
use mln_gfx::shaders::shader_defines::{ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};
use mln_gfx::uniform_buffer::{self, UniformBufferArray};
use mln_util::logging::{Event, Log};
use objc2_metal::MTLResourceOptions;

use crate::buffer_resource::BufferResource;
use crate::context::Context;
use crate::render_pass::RenderPass;

pub(crate) struct UniformBuffer {
    buffer: BufferResource,
}

impl UniformBuffer {
    pub(crate) fn new(buffer: BufferResource) -> Self {
        if let Some(context) = buffer.context() {
            let mut stats = context.rendering_stats();
            stats.num_uniform_buffers += 1;
            stats.mem_uniform_buffers += buffer.get_size_in_bytes() as i32;
        }
        UniformBuffer { buffer }
    }

    pub(crate) fn create(context: &Context, data: &[u8], persistent: bool) -> Self {
        UniformBuffer::new(BufferResource::new(
            context,
            Some(data),
            data.len(),
            MTLResourceOptions::StorageModeShared,
            /*is_index_buffer=*/ false,
            persistent,
        ))
    }

    pub(crate) fn get_buffer_resource(&self) -> &BufferResource {
        &self.buffer
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

/// Binds each buffer of `array` at its id, to the vertex and fragment functions
/// both except for the reserved vertex-only and fragment-only ids.
pub(crate) fn bind_uniform_buffer_array(array: &UniformBufferArray, render_pass: &RenderPass) {
    for id in 0..array.allocated_size() {
        let Some(uniform_buffer) = array.get(id) else {
            continue;
        };
        let Some(buffer) = uniform_buffer.downcast_ref::<UniformBuffer>() else {
            continue;
        };
        let resource = buffer.get_buffer_resource();
        if id != ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO {
            render_pass.bind_vertex(resource, 0, id, 0);
        }
        if id != ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO {
            render_pass.bind_fragment(resource, 0, id, 0);
        }
    }
}
