use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;

use crate::gfx::context::Device;

/// A buffer of [`Uniform`](mln_gfx::gpu::BufferUsage::Uniform) or
/// [`Storage`](mln_gfx::gpu::BufferUsage::Storage) usage.
pub(crate) type UniformBufferPtr = BufferPtr;

/// A collection of uniform buffers by id.
#[derive(Clone)]
pub(crate) struct UniformBufferArray {
    uniform_buffer_vector: [Option<UniformBufferPtr>; MAX_UBO_COUNT_PER_SHADER],
    epoch: u64,
}

thread_local! {
    /// The epoch the next change of any array takes.
    static NEXT_EPOCH: Cell<u64> = const { Cell::new(1) };
}

fn next_epoch() -> u64 {
    NEXT_EPOCH.with(|next| next.replace(next.get() + 1))
}

impl Default for UniformBufferArray {
    fn default() -> Self {
        UniformBufferArray { uniform_buffer_vector: Default::default(), epoch: next_epoch() }
    }
}

impl UniformBufferArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A number that changes whenever a buffer of the array is replaced,
    /// to one that no other array has had: bindings of the array's buffers
    /// made at one epoch bind them while it lasts.
    #[must_use]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
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
        let slot = &mut self.uniform_buffer_vector[id];
        if !same_buffer(slot.as_ref(), uniform_buffer.as_ref()) {
            *slot = uniform_buffer;
            self.epoch = next_epoch();
        }
        slot.as_ref()
    }

    /// Writes `data` to the uniform buffer `id`, first making one if there
    /// is none or `data` doesn't fit in it.
    pub(crate) fn create_or_update_pod<T: bytemuck::Pod>(&mut self, id: usize, data: &T, device: &mut Device) {
        self.write(id, bytemuck::bytes_of(data), false, device);
    }

    /// Writes `data` to the storage buffer `id`, first making one if there
    /// is none or `data` doesn't fit in it.
    pub(crate) fn write_storage(&mut self, id: usize, data: &[u8], device: &mut Device) {
        self.write(id, data, true, device);
    }

    fn write(&mut self, id: usize, data: &[u8], storage: bool, device: &mut Device) {
        let slot = &mut self.uniform_buffer_vector[id];
        let before = slot.as_ref().map(Rc::as_ptr);
        if storage {
            device.write_storage_buffer(slot, data);
        } else {
            device.write_uniform_buffer(slot, data);
        }
        if slot.as_ref().map(Rc::as_ptr) != before {
            self.epoch = next_epoch();
        }
    }
}

fn same_buffer(a: Option<&UniformBufferPtr>, b: Option<&UniformBufferPtr>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
