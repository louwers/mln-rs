//! The shapes of the descriptor sets of `shaders::layout`, and the pools
//! they are allocated from.

use std::collections::VecDeque;

use ash::vk;
use mln_gfx::shaders::shader_defines::{ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use crate::shaders::layout::{
    DRAWABLE_UNIFORM_COUNT, DescriptorSetType, GLOBAL_UNIFORM_COUNT, LAYER_STORAGE_COUNT, LAYER_UNIFORM_COUNT,
    layer_storage_binding,
};

/// The most buffers a set has.
const MAX_BUFFER_BINDINGS: usize = 8;
const _: () = assert!(
    GLOBAL_UNIFORM_COUNT <= MAX_BUFFER_BINDINGS
        && LAYER_STORAGE_COUNT + LAYER_UNIFORM_COUNT <= MAX_BUFFER_BINDINGS
        && DRAWABLE_UNIFORM_COUNT <= MAX_BUFFER_BINDINGS
);

/// The descriptors of a buffer set: `storage` storage buffers from binding
/// 0, then `uniform` uniform buffers.
#[derive(Clone, Copy)]
pub(crate) struct BufferSetShape {
    pub(crate) storage: usize,
    pub(crate) uniform: usize,
}

impl DescriptorSetType {
    /// The buffers of the type's sets; none for the image set.
    #[must_use]
    pub(crate) fn buffer_shape(self) -> BufferSetShape {
        match self {
            DescriptorSetType::Global => BufferSetShape { storage: 0, uniform: GLOBAL_UNIFORM_COUNT },
            DescriptorSetType::Layer => BufferSetShape { storage: LAYER_STORAGE_COUNT, uniform: LAYER_UNIFORM_COUNT },
            DescriptorSetType::DrawableUniform => BufferSetShape { storage: 0, uniform: DRAWABLE_UNIFORM_COUNT },
            DescriptorSetType::DrawableImage => BufferSetShape { storage: 0, uniform: 0 },
        }
    }

    /// The stages that read the buffer at `binding`
    /// (`buildUniformDescriptorSetLayout`): all but the fragment stage for a
    /// layer's vertex-only buffer, and all but the vertex stage for its
    /// fragment-only one.
    #[must_use]
    pub(crate) fn buffer_stages(self, binding: u32) -> vk::ShaderStageFlags {
        let mut stages = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        if self == DescriptorSetType::Layer {
            if layer_storage_binding(ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO) == Some(binding) {
                stages = vk::ShaderStageFlags::VERTEX;
            } else if layer_storage_binding(ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO) == Some(binding) {
                stages = vk::ShaderStageFlags::FRAGMENT;
            }
        }
        stages
    }
}

pub(crate) struct PoolInfo {
    pub(crate) pool: vk::DescriptorPool,
    remaining_sets: u32,
    /// The sets given back, each a set per frame in flight.
    unused_sets: VecDeque<Vec<vk::DescriptorSet>>,
}

/// The pools the sets of a type come from. Each new pool is `grow_factor` times
/// the last.
pub(crate) struct DescriptorPoolGrowable {
    max_sets: u32,
    descriptor_storage_per_set: u32,
    descriptor_uniforms_per_set: u32,
    descriptor_textures_per_set: u32,
    grow_factor: f32,
    pub(crate) pools: Vec<PoolInfo>,
    current_pool_index: Option<usize>,
}

impl DescriptorPoolGrowable {
    #[must_use]
    pub(crate) fn new(
        max_sets: u32,
        descriptor_storage_per_set: u32,
        descriptor_uniforms_per_set: u32,
        descriptor_textures_per_set: u32,
    ) -> Self {
        DescriptorPoolGrowable {
            max_sets,
            descriptor_storage_per_set,
            descriptor_uniforms_per_set,
            descriptor_textures_per_set,
            grow_factor: 1.5,
            pools: Vec::new(),
            current_pool_index: None,
        }
    }

    fn create_descriptor_pool(&mut self, device: &ash::Device) -> Option<usize> {
        let max_sets = (self.max_sets as f32 * self.grow_factor.powi(self.pools.len() as i32)) as u32;
        let mut sizes = Vec::new();
        for (count, type_) in [
            (self.descriptor_storage_per_set, vk::DescriptorType::STORAGE_BUFFER),
            (self.descriptor_uniforms_per_set, vk::DescriptorType::UNIFORM_BUFFER),
            (self.descriptor_textures_per_set, vk::DescriptorType::SAMPLED_IMAGE),
            (self.descriptor_textures_per_set, vk::DescriptorType::SAMPLER),
        ] {
            if count > 0 {
                sizes.push(vk::DescriptorPoolSize { ty: type_, descriptor_count: max_sets * count });
            }
        }
        let create_info = vk::DescriptorPoolCreateInfo::default().pool_sizes(&sizes).max_sets(max_sets);
        // SAFETY: the create info is valid for the device.
        let pool = unsafe { device.create_descriptor_pool(&create_info, None) }.ok()?;
        self.pools.push(PoolInfo { pool, remaining_sets: max_sets, unused_sets: VecDeque::new() });
        self.current_pool_index = Some(self.pools.len() - 1);
        self.current_pool_index
    }

    /// Allocates a set per frame in flight with `layout`. Returns them and
    /// the index of the pool they are from.
    pub(crate) fn allocate(
        &mut self,
        device: &ash::Device,
        layout: vk::DescriptorSetLayout,
        frame_count: usize,
    ) -> Option<(Vec<vk::DescriptorSet>, usize)> {
        let has_room = |pool: &PoolInfo| !pool.unused_sets.is_empty() || pool.remaining_sets as usize >= frame_count;
        let mut index = self.current_pool_index.filter(|&index| has_room(&self.pools[index]));
        if index.is_none() {
            // Find a pool that has unused allocated descriptor sets, then
            // one that has available memory to allocate more descriptor
            // sets.
            index = self
                .pools
                .iter()
                .position(|pool| !pool.unused_sets.is_empty())
                .or_else(|| self.pools.iter().position(|pool| pool.remaining_sets as usize >= frame_count))
                .or_else(|| self.create_descriptor_pool(device));
            self.current_pool_index = index;
        }
        let index = index?;
        let pool = &mut self.pools[index];
        if let Some(sets) = pool.unused_sets.pop_front() {
            return Some((sets, index));
        }
        let layouts = vec![layout; frame_count];
        let allocate_info = vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool.pool).set_layouts(&layouts);
        // SAFETY: the pool has `frame_count` sets of the layout's
        // descriptors left.
        let sets = unsafe { device.allocate_descriptor_sets(&allocate_info) }.ok()?;
        pool.remaining_sets -= sets.len() as u32;
        Some((sets, index))
    }

    /// Takes back the sets of `DescriptorSet::allocate`, for the next one.
    pub(crate) fn give_back(&mut self, pool_index: usize, sets: Vec<vk::DescriptorSet>) {
        if let Some(pool) = self.pools.get_mut(pool_index) {
            pool.unused_sets.push_back(sets);
        }
    }
}
