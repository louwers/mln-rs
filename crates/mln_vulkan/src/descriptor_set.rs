//! The descriptor set layouts of `mln_glsl::layout`, the pipeline layout
//! made of them, and the pools the sets are allocated from.

use ash::vk;
use mln_gfx::shaders::shader_defines::ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
use mln_glsl::layout::{
    BufferSetShape, DRAWABLE_UNIFORM_COUNT, DRAWABLE_VERTEX_COUNT, DescriptorSetType, GLOBAL_UNIFORM_COUNT,
    IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT, LAYER_UNIFORM_COUNT, image_sampler_binding, image_texture_binding,
    layer_storage_binding,
};

use crate::device::Device;

/// The most buffers a set has.
const MAX_BUFFER_BINDINGS: usize = 8;
const _: () = assert!(
    GLOBAL_UNIFORM_COUNT <= MAX_BUFFER_BINDINGS
        && LAYER_STORAGE_COUNT + LAYER_UNIFORM_COUNT <= MAX_BUFFER_BINDINGS
        && DRAWABLE_UNIFORM_COUNT + DRAWABLE_VERTEX_COUNT <= MAX_BUFFER_BINDINGS
);

/// The buffers of the sets of `type_`: with `vertex_buffers`, the storage
/// buffers that bind vertex buffers too (`Caps::binds_vertex_buffers`).
#[must_use]
pub(crate) const fn buffer_shape(type_: DescriptorSetType, vertex_buffers: bool) -> BufferSetShape {
    let shape = type_.buffer_shape();
    if vertex_buffers { shape } else { BufferSetShape { vertex: 0, ..shape } }
}

/// How many storage buffers the vertex stage of a pipeline reads at most
/// with the bindings of vertex buffers, which a device's
/// `maxPerStageDescriptorStorageBuffers` has to allow: the layer set's, and
/// the vertex buffers.
pub(crate) const VERTEX_STAGE_STORAGE_BUFFERS: usize = LAYER_STORAGE_COUNT + DRAWABLE_VERTEX_COUNT;

/// The stages that read the buffer at `binding` of a set of `type_`: all
/// but the fragment stage for a layer's vertex-only buffer and for a vertex
/// buffer. A layer's fragment-only buffer is the ground of a standing
/// layer's terrain variant too, which its vertex stage reads.
fn buffer_stages(type_: DescriptorSetType, binding: u32) -> vk::ShaderStageFlags {
    let shape = type_.buffer_shape();
    let vertex_only = binding as usize >= shape.storage + shape.uniform
        || (type_ == DescriptorSetType::Layer
            && layer_storage_binding(ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO) == Some(binding));
    if vertex_only {
        vk::ShaderStageFlags::VERTEX
    } else {
        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT
    }
}

pub(crate) struct PoolInfo {
    pub(crate) pool: vk::DescriptorPool,
    remaining_sets: u32,
    /// The sets given back, for the next allocations.
    unused_sets: Vec<vk::DescriptorSet>,
}

/// The pools the sets of a type come from. Each new pool is `GROW_FACTOR`
/// times the size of the last.
pub(crate) struct DescriptorPoolGrowable {
    max_sets: u32,
    descriptor_storage_per_set: u32,
    descriptor_uniforms_per_set: u32,
    descriptor_textures_per_set: u32,
    pub(crate) pools: Vec<PoolInfo>,
    current_pool_index: Option<usize>,
}

const GROW_FACTOR: f32 = 1.5;

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
            pools: Vec::new(),
            current_pool_index: None,
        }
    }

    fn create_descriptor_pool(&mut self, device: &ash::Device) -> Option<usize> {
        let max_sets = (self.max_sets as f32 * GROW_FACTOR.powi(self.pools.len() as i32)) as u32;
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
        self.pools.push(PoolInfo { pool, remaining_sets: max_sets, unused_sets: Vec::new() });
        self.current_pool_index = Some(self.pools.len() - 1);
        self.current_pool_index
    }

    /// Allocates a set with `layout`. Returns it and the index of the pool
    /// it is from.
    pub(crate) fn allocate(
        &mut self,
        device: &ash::Device,
        layout: vk::DescriptorSetLayout,
    ) -> Option<(vk::DescriptorSet, usize)> {
        let has_room = |pool: &PoolInfo| !pool.unused_sets.is_empty() || pool.remaining_sets > 0;
        let mut index = self.current_pool_index.filter(|&index| has_room(&self.pools[index]));
        if index.is_none() {
            // A pool with sets given back, then one with room for more, then
            // a new one.
            index = self
                .pools
                .iter()
                .position(|pool| !pool.unused_sets.is_empty())
                .or_else(|| self.pools.iter().position(|pool| pool.remaining_sets > 0))
                .or_else(|| self.create_descriptor_pool(device));
            self.current_pool_index = index;
        }
        let index = index?;
        let pool = &mut self.pools[index];
        if let Some(set) = pool.unused_sets.pop() {
            return Some((set, index));
        }
        let layouts = [layout];
        let allocate_info = vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool.pool).set_layouts(&layouts);
        // SAFETY: the pool has a set of the layout's descriptors left.
        let set = *unsafe { device.allocate_descriptor_sets(&allocate_info) }.ok()?.first()?;
        pool.remaining_sets -= 1;
        Some((set, index))
    }

    /// Takes back a set of [`allocate`](Self::allocate), for the next one.
    pub(crate) fn give_back(&mut self, pool_index: usize, set: vk::DescriptorSet) {
        if let Some(pool) = self.pools.get_mut(pool_index) {
            pool.unused_sets.push(set);
        }
    }
}

/// The layout of the sets of `type_`, with the bindings of vertex buffers
/// if `vertex_buffers`.
pub(crate) fn build_descriptor_set_layout(
    device: &Device,
    type_: DescriptorSetType,
    vertex_buffers: bool,
) -> Result<vk::DescriptorSetLayout, String> {
    let mut bindings = Vec::new();
    if type_ == DescriptorSetType::DrawableImage {
        // A vertex stage fetches texels, without a sampler.
        let texture_stages = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        for index in 0..IMAGE_TEXTURE_COUNT {
            for (binding, descriptor_type, stages) in [
                (image_texture_binding(index), vk::DescriptorType::SAMPLED_IMAGE, texture_stages),
                (image_sampler_binding(index), vk::DescriptorType::SAMPLER, vk::ShaderStageFlags::FRAGMENT),
            ] {
                bindings.push(
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .stage_flags(stages)
                        .descriptor_type(descriptor_type)
                        .descriptor_count(1),
                );
            }
        }
    } else {
        let shape = buffer_shape(type_, vertex_buffers);
        for binding in 0..shape.len() as u32 {
            let descriptor_type = if shape.is_storage(binding as usize) {
                vk::DescriptorType::STORAGE_BUFFER
            } else {
                vk::DescriptorType::UNIFORM_BUFFER
            };
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .stage_flags(buffer_stages(type_, binding))
                    .descriptor_type(descriptor_type)
                    .descriptor_count(1),
            );
        }
    }
    let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    // SAFETY: the create info is valid for the device.
    let layout = unsafe { device.shared.device().create_descriptor_set_layout(&create_info, None) }
        .map_err(|e| format!("Failed to create a descriptor set layout: {e}"))?;
    device.set_debug_name(layout, &format!("{type_:?}DescriptorSetLayout"));
    Ok(layout)
}

/// A pipeline layout of the sets `layouts` and `push_constant_size` bytes
/// of push constants, which both stages read.
pub(crate) fn build_pipeline_layout(
    device: &Device,
    layouts: &[vk::DescriptorSetLayout],
    push_constant_size: u32,
    name: &str,
) -> Result<vk::PipelineLayout, String> {
    let push_constant = [vk::PushConstantRange::default()
        .size(push_constant_size)
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];
    let create_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant).set_layouts(layouts);
    // SAFETY: the layouts are live, and the push constants fit the 128
    // bytes every device has.
    let layout = unsafe { device.shared.device().create_pipeline_layout(&create_info, None) }
        .map_err(|e| format!("Failed to create a pipeline layout: {e}"))?;
    device.set_debug_name(layout, name);
    Ok(layout)
}
