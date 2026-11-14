//! The table of `globalIndex` values: what tells a layer's draws apart is in
//! a group of its own, one table of indices bound at the draw's dynamic
//! offset.

use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;

/// The size of WGSL's `GlobalIndexUBO`: a `u32`, padded by a `vec3<u32>`.
const INDEX_ENTRY_SIZE: u64 = 32;

/// Entries of the first table, which grows to the next power of two that
/// holds the index a draw asks for.
const INITIAL_INDEX_CAPACITY: u32 = 256;

/// The group of `globalIndex`, at binding [`ID_GLOBAL_UBO_INDEX`]: one
/// uniform buffer whose entry `i`, at `i` strides, holds `i`, bound at the
/// dynamic offset of the draw's index.
pub(crate) struct IndexTable {
    layout: wgpu::BindGroupLayout,
    /// The distance between entries, a multiple of the device's uniform
    /// buffer offset alignment.
    stride: u32,
    /// The buffer, the bind group over it and the entries it holds.
    table: Option<(wgpu::Buffer, wgpu::BindGroup, u32)>,
}

impl IndexTable {
    pub(crate) fn new(device: &wgpu::Device) -> IndexTable {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UBO index"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: ID_GLOBAL_UBO_INDEX as u32,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(INDEX_ENTRY_SIZE),
                },
                count: None,
            }],
        });
        let alignment = device.limits().min_uniform_buffer_offset_alignment;
        let stride = (INDEX_ENTRY_SIZE as u32).next_multiple_of(alignment);
        IndexTable { layout, stride, table: None }
    }

    /// The layout of the group, which every shader's index group uses.
    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Makes the table hold `index`, writing a new buffer's entries with
    /// `queue`. Returns the size of the buffer it replaced (0 if none) and of
    /// the new one, if it made one.
    pub(crate) fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, index: u32) -> Option<(u64, u64)> {
        let replaced = match &self.table {
            Some((_, _, capacity)) if index < *capacity => return None,
            Some((buffer, _, _)) => buffer.size(),
            None => 0,
        };
        let capacity = index.saturating_add(1).next_power_of_two().max(INITIAL_INDEX_CAPACITY);
        let mut contents = vec![0u8; capacity as usize * self.stride as usize];
        for (i, entry) in contents.chunks_exact_mut(self.stride as usize).enumerate() {
            entry[..4].copy_from_slice(&(i as u32).to_ne_bytes());
        }
        // Written through the queue rather than mapped at creation: a
        // browser throws from `createBuffer` with `mappedAtCreation` once it
        // has lost the device, which would abort the instance.
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("UBO index table"),
            size: contents.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &contents);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UBO index"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: ID_GLOBAL_UBO_INDEX as u32,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(INDEX_ENTRY_SIZE),
                }),
            }],
        });
        let size = buffer.size();
        self.table = Some((buffer, bind_group, capacity));
        Some((replaced, size))
    }

    /// The bind group and the dynamic offset that select `index`, `None` if
    /// the table doesn't hold it (see [`reserve`](Self::reserve)).
    pub(crate) fn get(&self, index: u32) -> Option<(&wgpu::BindGroup, u32)> {
        let (_, bind_group, capacity) = self.table.as_ref()?;
        (index < *capacity).then(|| (bind_group, index * self.stride))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> (wgpu::Device, wgpu::Queue) {
        let device = crate::Device::new(None).expect("a WebGPU device");
        (device.device().clone(), device.queue().clone())
    }

    #[test]
    fn index_table_grows_to_hold_an_index() {
        let (device, queue) = device();
        let mut table = IndexTable::new(&device);
        assert!(table.get(0).is_none());

        let (replaced, size) = table.reserve(&device, &queue, 3).expect("a first buffer");
        assert_eq!(replaced, 0);
        assert_eq!(size, u64::from(INITIAL_INDEX_CAPACITY * table.stride));
        assert_eq!(table.reserve(&device, &queue, INITIAL_INDEX_CAPACITY - 1), None);
        let (_, offset) = table.get(3).unwrap();
        assert_eq!(offset, 3 * table.stride);
        assert!(table.get(INITIAL_INDEX_CAPACITY).is_none());

        assert_eq!(table.reserve(&device, &queue, INITIAL_INDEX_CAPACITY), Some((size, 2 * size)));
        assert!(table.get(INITIAL_INDEX_CAPACITY).is_some());
    }
}
