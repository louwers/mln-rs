//! The renderer's buffers of its backend's `Gpu`, each with one owner.

use std::rc::Rc;

use mln_gfx::gpu::{BufferId, BufferUsage};
use mln_gfx::rendering_stats::RenderingStats;

use crate::draw::release::{Release, ReleaseQueue};

/// The bytes a uniform or storage buffer takes are a multiple of this, so
/// that a binding covers a shader's whole struct where the renderer writes
/// less than it reads, and small buffers can share a larger one at offsets
/// any device can bind.
const UNIFORM_BUFFER_ALIGNMENT: usize = 256;

/// A buffer of the context's `Gpu`, which goes to the context's release
/// queue when it drops, and which the rendering stats count until the
/// context destroys it. Its owner drops the bindings and geometry made of
/// it no later than the buffer; draws that share one hold it in an `Rc`.
pub(crate) struct Buffer {
    id: BufferId,
    size: usize,
    usage: BufferUsage,
    queue: Rc<ReleaseQueue>,
}

impl Buffer {
    /// The buffer `id` of `usage`, which writes of up to `size` bytes fit
    /// in, and which goes to `queue` when it drops.
    pub(crate) fn new(id: BufferId, usage: BufferUsage, size: usize, queue: &Rc<ReleaseQueue>) -> Buffer {
        Buffer { id, size, usage, queue: Rc::clone(queue) }
    }

    #[must_use]
    pub(crate) fn id(&self) -> BufferId {
        self.id
    }

    /// The size of the data the buffer was made with. Writes may fill this
    /// much.
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub(crate) fn usage(&self) -> BufferUsage {
        self.usage
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.queue.push(Release::Buffer(self.id, self.usage, allocated_size(self.usage, self.size)));
    }
}

/// The label of a buffer of `usage`.
pub(crate) fn label(usage: BufferUsage) -> &'static str {
    match usage {
        BufferUsage::Vertex => "vertex buffer",
        BufferUsage::Index => "index buffer",
        BufferUsage::Uniform => "uniform buffer",
        BufferUsage::Storage => "storage buffer",
    }
}

/// The bytes the GPU buffer of a buffer of `usage` and `size` bytes takes.
pub(crate) fn allocated_size(usage: BufferUsage, size: usize) -> usize {
    match usage {
        BufferUsage::Vertex | BufferUsage::Index => size.max(1).next_multiple_of(4),
        BufferUsage::Uniform | BufferUsage::Storage => size.max(1).next_multiple_of(UNIFORM_BUFFER_ALIGNMENT),
    }
}

/// Adds `sign` buffers of `usage` taking `allocated` bytes each to the live
/// buffers of `stats`.
pub(crate) fn count(stats: &mut RenderingStats, usage: BufferUsage, allocated: usize, sign: i32) {
    let bytes = sign * allocated as i32;
    stats.num_buffers += sign;
    stats.mem_buffers += bytes;
    let (num, mem) = match usage {
        BufferUsage::Vertex => (&mut stats.num_vertex_buffers, &mut stats.mem_vertex_buffers),
        BufferUsage::Index => (&mut stats.num_index_buffers, &mut stats.mem_index_buffers),
        BufferUsage::Uniform | BufferUsage::Storage => (&mut stats.num_uniform_buffers, &mut stats.mem_uniform_buffers),
    };
    *num += sign;
    *mem += bytes;
}

/// Counts a write of `bytes` to a buffer of `usage` in `stats`. Every
/// nonempty write of a uniform or storage buffer counts; of a vertex or
/// index buffer, only an `update` of one made earlier.
pub(crate) fn count_write(stats: &mut RenderingStats, usage: BufferUsage, bytes: usize, update: bool) {
    if bytes == 0 {
        return;
    }
    let update_bytes = match usage {
        BufferUsage::Uniform | BufferUsage::Storage => {
            stats.num_uniform_updates += 1;
            stats.uniform_update_bytes = stats.uniform_update_bytes.wrapping_add(bytes);
            return;
        }
        _ if !update => return,
        BufferUsage::Vertex => &mut stats.vertex_update_bytes,
        BufferUsage::Index => &mut stats.index_update_bytes,
    };
    *update_bytes = update_bytes.wrapping_add(bytes);
    stats.buffer_updates += 1;
    stats.buffer_obj_updates += 1;
    stats.buffer_update_bytes = stats.buffer_update_bytes.wrapping_add(bytes);
}
