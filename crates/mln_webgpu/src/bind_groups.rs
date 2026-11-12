//! Bind groups that drawables share.
//!
//! Drawables of a layer bind the same layer buffers, so they share their bind
//! groups, and a render pass skips setting a group the previous draw set.
//! What tells the draws apart, `globalIndex`, is in a group of its own: one
//! table of indices for every drawable, bound at the draw's dynamic offset.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use wgpu::util::DeviceExt as _;

/// A resource a bind group entry binds. wgpu handles compare by identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum EntryResource {
    /// A buffer, and the offset and size bound.
    Buffer(wgpu::Buffer, u64, u64),
    Sampler(wgpu::Sampler),
    TextureView(wgpu::TextureView),
}

/// What a bind group is created from: its layout and the resource of each
/// binding.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct BindGroupKey {
    pub layout: wgpu::BindGroupLayout,
    pub resources: Vec<(u32, EntryResource)>,
}

type Entries = RefCell<HashMap<BindGroupKey, Weak<SharedBindGroup>>>;

/// A bind group of a [`BindGroupCache`], and what it was created from.
/// Dropping the last `Rc` takes it out of the cache.
pub(crate) struct SharedBindGroup {
    pub handle: wgpu::BindGroup,
    pub key: BindGroupKey,
    cache: Weak<Entries>,
}

impl Drop for SharedBindGroup {
    fn drop(&mut self) {
        let Some(cache) = self.cache.upgrade() else {
            return;
        };
        // The cache drops no bind group while it borrows its map, so the
        // borrow succeeds. The entry may be a newer bind group's if a dead
        // one was replaced, so remove it only if it's dead.
        if let Ok(mut entries) = cache.try_borrow_mut()
            && entries.get(&self.key).is_some_and(|entry| entry.strong_count() == 0)
        {
            entries.remove(&self.key);
        }
    }
}

/// Bind groups by what they are created from, each kept while something
/// holds its `Rc`, so the cache keeps no resource alive itself.
#[derive(Default)]
pub(crate) struct BindGroupCache {
    entries: Rc<Entries>,
}

impl BindGroupCache {
    /// The live bind group created from `key`, or a new one `create` makes.
    pub fn get_or_create(
        &self,
        key: BindGroupKey,
        create: impl FnOnce(&BindGroupKey) -> wgpu::BindGroup,
    ) -> Rc<SharedBindGroup> {
        if let Some(shared) = self.entries.borrow().get(&key).and_then(Weak::upgrade) {
            return shared;
        }
        let handle = create(&key);
        let shared = Rc::new(SharedBindGroup { handle, key: key.clone(), cache: Rc::downgrade(&self.entries) });
        self.entries.borrow_mut().insert(key, Rc::downgrade(&shared));
        shared
    }

    /// How many bind groups are live.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.borrow().len()
    }
}

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
    pub fn new(device: &wgpu::Device) -> IndexTable {
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
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Makes the table hold `index`. Returns the size of the buffer it
    /// replaced (0 if none) and of the new one, if it made one.
    pub fn reserve(&mut self, device: &wgpu::Device, index: u32) -> Option<(u64, u64)> {
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
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UBO index table"),
            contents: &contents,
            usage: wgpu::BufferUsages::UNIFORM,
        });
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
    pub fn get(&self, index: u32) -> Option<(&wgpu::BindGroup, u32)> {
        let (_, bind_group, capacity) = self.table.as_ref()?;
        (index < *capacity).then(|| (bind_group, index * self.stride))
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::backend_scope::{BackendScope, ScopeType};
    use mln_gfx::renderer_backend::RendererBackend as _;
    use mln_util::size::Size;

    use super::*;
    use crate::HeadlessBackend;

    fn device(backend: &HeadlessBackend) -> wgpu::Device {
        let context = backend.get_context().as_any().downcast_ref::<crate::Context>().unwrap();
        context.get_backend().get_device().expect("a device")
    }

    fn uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        })
    }

    #[test]
    fn bind_groups_are_shared_while_held() {
        let backend = HeadlessBackend::create(Size { width: 4, height: 4 });
        let _scope = BackendScope::new(&backend, ScopeType::Explicit);
        let device = device(&backend);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let key = |buffer: &wgpu::Buffer| BindGroupKey {
            layout: layout.clone(),
            resources: vec![(0, EntryResource::Buffer(buffer.clone(), 0, 256))],
        };
        let create = |key: &BindGroupKey| {
            let EntryResource::Buffer(buffer, _, _) = &key.resources[0].1 else { unreachable!() };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &key.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            })
        };
        let (a, b) = (uniform_buffer(&device), uniform_buffer(&device));
        let cache = BindGroupCache::default();

        let first = cache.get_or_create(key(&a), create);
        let again = cache.get_or_create(key(&a), |_| panic!("the bind group is live"));
        assert!(Rc::ptr_eq(&first, &again));
        let other = cache.get_or_create(key(&b), create);
        assert_ne!(first.handle, other.handle);
        assert_eq!(cache.len(), 2);

        drop((first, again));
        assert_eq!(cache.len(), 1, "a bind group no one holds leaves the cache");
        let made = cache.get_or_create(key(&a), create);
        assert_eq!(cache.len(), 2);
        drop((made, other));
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn index_table_grows_to_hold_an_index() {
        let backend = HeadlessBackend::create(Size { width: 4, height: 4 });
        let _scope = BackendScope::new(&backend, ScopeType::Explicit);
        let device = device(&backend);
        let mut table = IndexTable::new(&device);
        assert!(table.get(0).is_none());

        let (replaced, size) = table.reserve(&device, 3).expect("a first buffer");
        assert_eq!(replaced, 0);
        assert_eq!(size, u64::from(INITIAL_INDEX_CAPACITY * table.stride));
        assert_eq!(table.reserve(&device, INITIAL_INDEX_CAPACITY - 1), None);
        let (_, offset) = table.get(3).unwrap();
        assert_eq!(offset, 3 * table.stride);
        assert!(table.get(INITIAL_INDEX_CAPACITY).is_none());

        assert_eq!(table.reserve(&device, INITIAL_INDEX_CAPACITY), Some((size, 2 * size)));
        assert!(table.get(INITIAL_INDEX_CAPACITY).is_some());
    }
}
