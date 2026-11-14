use std::cell::Cell;
use std::rc::Rc;

use crate::context::Context;
use crate::gpu::{BufferDesc, BufferId, BufferUsage, Gpu, GpuError};
use crate::rendering_stats::RenderingStats;
use crate::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;

/// The bytes a uniform or storage buffer takes are a multiple of this, so
/// that a binding covers a shader's whole struct where the renderer writes
/// less than it reads, and small buffers can share a larger one at offsets
/// any device can bind.
pub const UNIFORM_BUFFER_ALIGNMENT: usize = 256;

/// A uniform or storage buffer of the renderer: a buffer of the backend's
/// [`Gpu`], which goes back to the context to destroy when the last handle
/// to it drops (see [`Released`]).
pub struct UniformBuffer {
    id: BufferId,
    size: usize,
    storage: bool,
    released: Rc<Released>,
}

impl UniformBuffer {
    /// Makes a buffer holding `data`, of [`Storage`](BufferUsage::Storage)
    /// usage if `storage`, else [`Uniform`](BufferUsage::Uniform), which
    /// goes back to `released` when dropped.
    pub(crate) fn create(
        gpu: &mut dyn Gpu,
        released: &Rc<Released>,
        stats: &mut RenderingStats,
        data: &[u8],
        storage: bool,
    ) -> Result<UniformBufferPtr, GpuError> {
        let allocated = allocated_size(data.len());
        let id = gpu.create_buffer(&BufferDesc {
            label: if storage { "storage buffer" } else { "uniform buffer" },
            usage: if storage { BufferUsage::Storage } else { BufferUsage::Uniform },
            size: allocated as u64,
        })?;
        // Counted before the write, whose failure drops the buffer, which
        // releases it and takes it out of the stats.
        stats.num_buffers += 1;
        stats.total_buffers += 1;
        stats.total_buffer_objs += 1;
        stats.mem_buffers += allocated as i32;
        stats.num_uniform_buffers += 1;
        stats.mem_uniform_buffers += allocated as i32;
        let buffer = UniformBuffer { id, size: data.len(), storage, released: Rc::clone(released) };
        buffer.write(gpu, stats, data)?;
        Ok(Rc::new(buffer))
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

    /// Whether the buffer binds as a storage buffer, which a layer's
    /// drawables index with their UBO index.
    #[must_use]
    pub fn is_storage(&self) -> bool {
        self.storage
    }

    /// Writes `data` at the start of the buffer. Fails if `data` is longer
    /// than [`size`](Self::size).
    pub(crate) fn write(&self, gpu: &mut dyn Gpu, stats: &mut RenderingStats, data: &[u8]) -> Result<(), GpuError> {
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
        stats.num_uniform_updates += 1;
        stats.uniform_update_bytes = stats.uniform_update_bytes.wrapping_add(data.len());
        Ok(())
    }
}

impl Drop for UniformBuffer {
    fn drop(&mut self) {
        let mut buffers = self.released.buffers.take();
        buffers.push((self.id, allocated_size(self.size)));
        self.released.buffers.set(buffers);
    }
}

/// The bytes the GPU buffer of a uniform buffer of `size` bytes takes.
fn allocated_size(size: usize) -> usize {
    size.max(1).next_multiple_of(UNIFORM_BUFFER_ALIGNMENT)
}

pub type UniformBufferPtr = Rc<UniformBuffer>;

/// The buffers the renderer dropped. A backend destroys them with
/// [`Context::destroy_released`] once it has submitted the frame that last
/// used them, so a buffer dropped while a frame is recorded outlives that
/// frame's commands.
#[derive(Default)]
pub struct Released {
    /// Each buffer and the bytes it took.
    buffers: Cell<Vec<(BufferId, usize)>>,
}

impl Released {
    /// Destroys the buffers released so far, taking them out of `stats`.
    pub(crate) fn destroy(&self, gpu: &mut dyn Gpu, stats: &mut RenderingStats) {
        for (id, allocated) in self.buffers.take() {
            gpu.destroy_buffer(id);
            stats.num_buffers -= 1;
            stats.mem_buffers -= allocated as i32;
            stats.num_uniform_buffers -= 1;
            stats.mem_uniform_buffers -= allocated as i32;
        }
    }
}

/// A collection of uniform buffers by id.
#[derive(Clone, Default)]
pub struct UniformBufferArray {
    uniform_buffer_vector: [Option<UniformBufferPtr>; MAX_UBO_COUNT_PER_SHADER],
}

impl UniformBufferArray {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of maximum allocated elements
    #[must_use]
    pub fn allocated_size(&self) -> usize {
        self.uniform_buffer_vector.len()
    }

    /// Get an uniform buffer element, or `None` if it doesn't exist.
    pub fn get(&self, id: usize) -> Option<&UniformBufferPtr> {
        self.uniform_buffer_vector.get(id).and_then(Option::as_ref)
    }

    /// Set a new uniform buffer element or replace the existing one.
    pub fn set(&mut self, id: usize, uniform_buffer: Option<UniformBufferPtr>) -> Option<&UniformBufferPtr> {
        debug_assert!(id < self.uniform_buffer_vector.len());
        if id >= self.uniform_buffer_vector.len() {
            return None;
        }
        self.uniform_buffer_vector[id] = uniform_buffer;
        self.uniform_buffer_vector[id].as_ref()
    }

    /// Writes `data` to the uniform buffer `id`, first making one if there
    /// is none or `data` doesn't fit in it.
    pub fn create_or_update_pod<T: bytemuck::Pod>(&mut self, id: usize, data: &T, context: &dyn Context) {
        context.write_uniform_buffer(&mut self.uniform_buffer_vector[id], bytemuck::bytes_of(data));
    }

    /// Copies every slot of `other`.
    pub fn assign_from(&mut self, other: &UniformBufferArray) {
        self.uniform_buffer_vector.clone_from(&other.uniform_buffer_vector);
    }
}
