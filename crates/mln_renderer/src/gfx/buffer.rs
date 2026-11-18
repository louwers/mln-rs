//! The renderer's buffers: each a buffer of the backend's `Gpu` in the
//! context's store ([`Resources`](crate::gfx::resources::Resources)), which
//! its owners and the bindings and geometry made of it hold as a
//! [`SharedBuffer`].

use mln_gfx::gpu::{BufferId, BufferUsage};
use mln_gfx::rendering_stats::RenderingStats;

use crate::gfx::store::{BufferKey, Shared};

/// The bytes a uniform or storage buffer takes are a multiple of this, so
/// that a binding covers a shader's whole struct where the renderer writes
/// less than it reads, and small buffers can share a larger one at offsets
/// any device can bind.
const UNIFORM_BUFFER_ALIGNMENT: usize = 256;

/// A buffer in the context's store, which the context destroys once its
/// last handle drops, and which the rendering stats count until then.
pub(crate) struct Buffer {
    pub(crate) id: BufferId,
    pub(crate) usage: BufferUsage,
    /// The bytes of its GPU buffer.
    pub(crate) allocated: usize,
}

/// What a buffer's handles read of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BufferInfo {
    id: BufferId,
    size: usize,
    usage: BufferUsage,
}

/// A buffer of the context's store, which stays while a clone of this does.
pub(crate) type SharedBuffer = Shared<BufferKey, BufferInfo>;

impl SharedBuffer {
    #[must_use]
    pub(crate) fn id(&self) -> BufferId {
        self.data().id
    }

    /// The size of the data the buffer was made with. Writes may fill this
    /// much.
    #[must_use]
    pub(crate) fn size(&self) -> usize {
        self.data().size
    }

    #[must_use]
    pub(crate) fn usage(&self) -> BufferUsage {
        self.data().usage
    }

    /// Whether the buffer binds as a storage buffer, which a layer's draws
    /// index with their index among its draws.
    #[must_use]
    pub(crate) fn is_storage(&self) -> bool {
        self.usage() == BufferUsage::Storage
    }
}

impl Buffer {
    /// A buffer of `usage` that writes of up to `size` bytes fit in, and
    /// what its handles read of it.
    pub(crate) fn new(id: BufferId, usage: BufferUsage, size: usize) -> (Buffer, BufferInfo) {
        (Buffer { id, usage, allocated: allocated_size(usage, size) }, BufferInfo { id, size, usage })
    }

    /// Adds the buffer to the live buffers of `stats`.
    pub(crate) fn count(&self, stats: &mut RenderingStats) {
        self.add_to(stats, 1);
        stats.total_buffers += 1;
        stats.total_buffer_objs += 1;
    }

    /// Takes the buffer out of the live buffers of `stats`.
    pub(crate) fn uncount(&self, stats: &mut RenderingStats) {
        self.add_to(stats, -1);
    }

    fn add_to(&self, stats: &mut RenderingStats, sign: i32) {
        let bytes = sign * self.allocated as i32;
        stats.num_buffers += sign;
        stats.mem_buffers += bytes;
        let (num, mem) = match self.usage {
            BufferUsage::Vertex => (&mut stats.num_vertex_buffers, &mut stats.mem_vertex_buffers),
            BufferUsage::Index => (&mut stats.num_index_buffers, &mut stats.mem_index_buffers),
            BufferUsage::Uniform | BufferUsage::Storage => {
                (&mut stats.num_uniform_buffers, &mut stats.mem_uniform_buffers)
            }
        };
        *num += sign;
        *mem += bytes;
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
