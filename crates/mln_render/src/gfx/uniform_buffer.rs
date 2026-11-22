use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::FIRST_VERTEX_BUFFER_ID;

use crate::gfx::buffer::Buffer;
use crate::gfx::release::Release;
use crate::gfx::resources::Device;

/// Uniform and storage buffers by id, which their array owns. A buffer the
/// array replaces or loses, or the array dropping, retires the array's
/// epoch, so that the context drops the bindings made of it before it
/// destroys the buffer.
#[derive(Default)]
pub(crate) struct UniformBufferArray {
    buffers: [Option<Buffer>; FIRST_VERTEX_BUFFER_ID],
    epoch: u64,
}

impl UniformBufferArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A number for the array's buffers: 0 until one is made, and whenever
    /// one is made, replaced or lost to a failed write, one that no other
    /// array of its device has had. Bindings of the array's buffers made at
    /// one epoch bind them while it lasts.
    #[must_use]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The buffer `id`, if the array has one.
    pub(crate) fn get(&self, id: usize) -> Option<&Buffer> {
        self.buffers.get(id).and_then(Option::as_ref)
    }

    /// Writes `data` to the uniform buffer `id`, first making one if there
    /// is none or `data` doesn't fit in it. Returns whether it replaced one,
    /// whose bindings the array's owner then drops.
    #[must_use]
    pub(crate) fn write_pod<T: bytemuck::Pod>(
        &mut self,
        gpu: &mut dyn Gpu,
        id: usize,
        data: &T,
        device: &mut Device,
    ) -> bool {
        self.write_uniform(gpu, id, bytemuck::bytes_of(data), device)
    }

    /// Writes `data`, the bytes of a block, to the uniform buffer `id`, as
    /// [`write_pod`](Self::write_pod) does.
    pub(crate) fn write_uniform(&mut self, gpu: &mut dyn Gpu, id: usize, data: &[u8], device: &mut Device) -> bool {
        self.write(gpu, id, data, false, device)
    }

    /// Writes `data` to the storage buffer `id`, as
    /// [`write_pod`](Self::write_pod) does.
    pub(crate) fn write_storage(&mut self, gpu: &mut dyn Gpu, id: usize, data: &[u8], device: &mut Device) -> bool {
        self.write(gpu, id, data, true, device)
    }

    fn write(&mut self, gpu: &mut dyn Gpu, id: usize, data: &[u8], storage: bool, device: &mut Device) -> bool {
        let slot = &mut self.buffers[id];
        let before = slot.as_ref().map(Buffer::id);
        let replaced = device.write_uniforms(gpu, slot, data, storage);
        if slot.as_ref().map(Buffer::id) != before {
            if before.is_some() {
                device.retire_uniform_epoch(self.epoch);
            }
            self.epoch = device.next_uniform_epoch();
        }
        replaced
    }
}

impl Drop for UniformBufferArray {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffers.iter().flatten().next() {
            buffer.release(Release::Epoch(self.epoch));
        }
    }
}
