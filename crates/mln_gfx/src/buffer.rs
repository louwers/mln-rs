use std::rc::Rc;

use crate::gpu::{BufferDesc, BufferId, BufferUsage, Gpu, GpuError};
use crate::released::Released;
use crate::rendering_stats::RenderingStats;

/// The bytes a uniform or storage buffer takes are a multiple of this, so
/// that a binding covers a shader's whole struct where the renderer writes
/// less than it reads, and small buffers can share a larger one at offsets
/// any device can bind.
const UNIFORM_BUFFER_ALIGNMENT: usize = 256;

/// A buffer of the renderer: a buffer of the backend's [`Gpu`], which goes
/// to the renderer's release queue when the last handle to it drops (see
/// [`Released`]), and which the renderer's rendering stats count.
pub struct Buffer {
    id: BufferId,
    size: usize,
    usage: BufferUsage,
    released: Rc<Released>,
}

pub type BufferPtr = Rc<Buffer>;

impl Buffer {
    /// Makes a buffer of `gpu` of `usage` holding `data`, which goes to
    /// `released` once dropped, and counts it in `stats`.
    pub fn create(
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        stats: &mut RenderingStats,
        usage: BufferUsage,
        data: &[u8],
    ) -> Result<BufferPtr, GpuError> {
        let buffer = Buffer::allocate(gpu, released, stats, usage, data.len())?;
        buffer.write_bytes(gpu, data)?;
        buffer.count_write(stats, data.len(), false);
        Ok(buffer)
    }

    /// Makes a buffer of `usage` holding the bytes of `data`, written from
    /// the JS array they're in.
    #[cfg(target_family = "wasm")]
    pub fn create_from_host(
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        stats: &mut RenderingStats,
        usage: BufferUsage,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<BufferPtr, GpuError> {
        let buffer = Buffer::allocate(gpu, released, stats, usage, data.len())?;
        if !data.is_empty() {
            gpu.write_buffer_from_host(buffer.id, 0, data)?;
        }
        Ok(buffer)
    }

    /// A buffer of `usage` that writes of up to `size` bytes fit in.
    fn allocate(
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        stats: &mut RenderingStats,
        usage: BufferUsage,
        size: usize,
    ) -> Result<BufferPtr, GpuError> {
        let allocated = allocated_size(usage, size);
        let label = match usage {
            BufferUsage::Vertex => "vertex buffer",
            BufferUsage::Index => "index buffer",
            BufferUsage::Uniform => "uniform buffer",
            BufferUsage::Storage => "storage buffer",
        };
        let id = gpu.create_buffer(&BufferDesc { label, usage, size: allocated as u64 })?;
        // Counted before the first write, whose failure drops the buffer,
        // which releases it and takes it out of the stats.
        count(stats, usage, allocated as i32);
        stats.total_buffers += 1;
        stats.total_buffer_objs += 1;
        Ok(Rc::new(Buffer { id, size, usage, released: Rc::clone(released) }))
    }

    #[must_use]
    pub fn id(&self) -> BufferId {
        self.id
    }

    /// The size of the data the buffer was made with. Writes may fill this
    /// much.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub fn usage(&self) -> BufferUsage {
        self.usage
    }

    /// Whether the buffer binds as a storage buffer, which a layer's
    /// drawables index with their UBO index.
    #[must_use]
    pub fn is_storage(&self) -> bool {
        self.usage == BufferUsage::Storage
    }

    /// Writes `data` at the start of the buffer, of `gpu`, and counts the
    /// write in `stats`. Fails if `data` is longer than
    /// [`size`](Self::size).
    pub fn write(&self, gpu: &mut dyn Gpu, stats: &mut RenderingStats, data: &[u8]) -> Result<(), GpuError> {
        self.write_bytes(gpu, data)?;
        self.count_write(stats, data.len(), true);
        Ok(())
    }

    /// [`write`](Self::write) without counting it in the stats.
    fn write_bytes(&self, gpu: &mut dyn Gpu, data: &[u8]) -> Result<(), GpuError> {
        if data.len() > self.size {
            return Err(GpuError::Invalid(format!("a write of {} bytes to a buffer of {}", data.len(), self.size)));
        }
        if data.is_empty() {
            return Ok(());
        }
        // `Gpu::write_buffer` takes whole 4-byte words.
        if data.len().is_multiple_of(4) {
            gpu.write_buffer(self.id, 0, data)?;
        } else {
            let mut padded = data.to_vec();
            padded.resize(data.len().next_multiple_of(4), 0);
            gpu.write_buffer(self.id, 0, &padded)?;
        }
        Ok(())
    }

    /// Counts a write of `bytes` in `stats`. Every nonempty write of a
    /// uniform or storage buffer counts; of a vertex or index buffer, only an
    /// `update` of one made earlier.
    fn count_write(&self, stats: &mut RenderingStats, bytes: usize, update: bool) {
        if bytes == 0 {
            return;
        }
        let update_bytes = match self.usage {
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
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.released.buffer(self.id, self.usage, allocated_size(self.usage, self.size));
    }
}

/// The bytes the GPU buffer of a buffer of `usage` and `size` bytes takes.
fn allocated_size(usage: BufferUsage, size: usize) -> usize {
    match usage {
        BufferUsage::Vertex | BufferUsage::Index => size.max(1).next_multiple_of(4),
        BufferUsage::Uniform | BufferUsage::Storage => size.max(1).next_multiple_of(UNIFORM_BUFFER_ALIGNMENT),
    }
}

/// Adds a new buffer of `usage` and `bytes` to the live buffers of `stats`.
fn count(stats: &mut RenderingStats, usage: BufferUsage, bytes: i32) {
    stats.num_buffers += 1;
    stats.mem_buffers += bytes;
    let (num, mem) = match usage {
        BufferUsage::Vertex => (&mut stats.num_vertex_buffers, &mut stats.mem_vertex_buffers),
        BufferUsage::Index => (&mut stats.num_index_buffers, &mut stats.mem_index_buffers),
        BufferUsage::Uniform | BufferUsage::Storage => (&mut stats.num_uniform_buffers, &mut stats.mem_uniform_buffers),
    };
    *num += 1;
    *mem += bytes;
}
