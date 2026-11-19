use std::cell::Cell;

use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;

use crate::gfx::buffer::Buffer;
use crate::gfx::resources::Device;

/// Uniform and storage buffers by id, which their array owns: the owner of
/// the array drops the bindings made of a buffer that it replaces.
pub(crate) struct UniformBufferArray {
    buffers: [Option<Buffer>; MAX_UBO_COUNT_PER_SHADER],
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
        UniformBufferArray { buffers: Default::default(), epoch: next_epoch() }
    }
}

impl UniformBufferArray {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A number that changes whenever a buffer of the array is made or
    /// replaced, to one that no other array has had: bindings of the
    /// array's buffers made at one epoch bind them while it lasts.
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
        self.write(gpu, id, bytemuck::bytes_of(data), false, device)
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
            self.epoch = next_epoch();
        }
        replaced
    }
}
