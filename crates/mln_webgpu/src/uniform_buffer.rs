// Ports include/mln/webgpu/uniform_buffer.hpp and src/mln/webgpu/uniform_buffer.cpp
//
// `webgpu::UniformBufferArray` only overrides `bind` (a no-op: bind groups
// are built per drawable in `Drawable::draw`) and `copy`, so the port uses
// `gfx::UniformBufferArray` directly.

use std::any::Any;
use std::cell::Cell;
use std::rc::{Rc, Weak};

use mln_gfx::uniform_buffer;

use crate::context::Context;
use crate::renderer_backend::BackendImpl;

/// WebGPU's largest allowed `minUniformBufferOffsetAlignment`, so a valid
/// uniform binding offset on any device.
pub(crate) const UNIFORM_BUFFER_ALIGNMENT: usize = 256;

const fn aligned_uniform_size(size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    size.div_ceil(UNIFORM_BUFFER_ALIGNMENT) * UNIFORM_BUFFER_ALIGNMENT
}

pub struct UniformBuffer {
    /// The size of the data, which `get_size` reports. The GPU buffer is
    /// padded to 256 bytes. C++ reports the padded size, so
    /// `UniformBufferArray::createOrUpdate`, which compares it to the data
    /// size, creates a new GPU buffer on every update of a UBO whose size
    /// isn't a multiple of 256 bytes. Reporting the data size updates it in
    /// place. Bind groups still bind the whole padded buffer, because some
    /// UBOs hold less data than their shader struct.
    size: usize,
    /// C++ keeps `Context&`; only device and queue are used. Holding the
    /// backend state rather than `Rc<Context>` avoids an Rc cycle through the
    /// context's global uniform buffer array.
    backend: Rc<BackendImpl>,
    /// The context whose stats count this buffer. Weak for the same reason;
    /// a buffer the context drops has no stats left to update.
    context: Weak<Context>,
    /// The GPU buffer, which other uniform buffers may share, and where in it
    /// this one's padded bytes start.
    buffer: Option<wgpu::Buffer>,
    offset: u64,
    /// The end of the bytes that may not be zeros: an update ending there or
    /// later can write the padding's zeros with it.
    written: Cell<usize>,
}

impl UniformBuffer {
    /// `UniformBuffer(context, data, size)`; `data == None` is the C++ null
    /// pointer.
    pub fn new(context: &Context, data: Option<&[u8]>, size: usize) -> Self {
        Self::with_backend(context.get_backend().clone(), context.weak(), data, size)
    }

    fn with_backend(backend: Rc<BackendImpl>, context: Weak<Context>, data: Option<&[u8]>, size: usize) -> Self {
        let aligned_size = aligned_uniform_size(size);
        let mut buffer = None;
        let mut offset = 0;
        let mut written = 0;
        if aligned_size > 0 {
            // C++ creates the buffer mapped and copies the data (zero padded);
            // a queue write of the data has the same effect for the next
            // submission.
            let (b, at) = backend.allocate_uniform_buffer(aligned_size as u64);
            if let Some(data) = data {
                backend.write_buffer_padded(&b, at, &data[..size], aligned_size as u64);
                written = size;
            }
            if let Some(context) = context.upgrade() {
                context.count_buffer_created(aligned_size as u64);
                let mut stats = context.rendering_stats_mut();
                stats.num_uniform_buffers += 1;
                stats.mem_uniform_buffers += aligned_size as i32;
            }
            buffer = Some(b);
            offset = at;
        }
        UniformBuffer { size, backend, context, buffer, offset, written: Cell::new(written) }
    }

    pub fn get_buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }

    /// Where this uniform buffer starts in [`get_buffer`](Self::get_buffer).
    pub fn get_offset(&self) -> u64 {
        self.offset
    }

    /// The size of this uniform buffer in its GPU buffer: the data's, padded
    /// to 256 bytes.
    pub fn get_aligned_size(&self) -> u64 {
        aligned_uniform_size(self.size) as u64
    }
}

impl Drop for UniformBuffer {
    fn drop(&mut self) {
        if self.buffer.is_some()
            && let Some(context) = self.context.upgrade()
        {
            let size = self.get_aligned_size();
            context.count_buffer_released(size);
            let mut stats = context.rendering_stats_mut();
            stats.num_uniform_buffers -= 1;
            stats.mem_uniform_buffers -= size as i32;
        }
    }
}

impl uniform_buffer::UniformBuffer for UniformBuffer {
    fn update(&self, data: &[u8]) {
        debug_assert!(data.len() <= aligned_uniform_size(self.size), "update larger than the uniform buffer");
        if !data.is_empty()
            && let Some(buffer) = &self.buffer
        {
            let written = self.written.get();
            if data.len() >= written {
                self.backend.write_buffer_padded(buffer, self.offset, data, self.get_aligned_size());
            } else {
                self.backend.write_buffer(buffer, self.offset, data);
            }
            self.written.set(written.max(data.len()));
            if let Some(context) = self.context.upgrade() {
                context.count_buffer_update(data.len());
                let mut stats = context.rendering_stats_mut();
                stats.num_uniform_updates += 1;
                stats.uniform_update_bytes = stats.uniform_update_bytes.wrapping_add(data.len());
            }
        }
    }

    fn get_size(&self) -> usize {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
