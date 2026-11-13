// `DescriptorSetType` is in `shaders::layout`, with the bindings.
//
// Differences from C++:
// - C++ marks a set dirty when a buffer or texture of its owner is set, and
//   then writes all of it. The gfx uniform buffer array of the port has no
//   hook for that, so a set remembers what it holds for each frame in flight
//   and writes what differs when it is bound.
// - A set that changes after the frame being recorded has bound it is
//   replaced by a new one: writing it would invalidate the commands that
//   use it.
// - An image set has a sampled image and a sampler for each texture (see
//   `shaders::layout`).

use std::collections::VecDeque;
use std::rc::Weak;

use ash::vk;
use mln_gfx::drawable::Textures;
use mln_gfx::shaders::shader_defines::{
    ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO, LAYER_SSBO_START_ID,
};
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gfx::uniform_buffer::UniformBufferArray;

use crate::context::Context;
use crate::shaders::layout::{
    DRAWABLE_UNIFORM_COUNT, DescriptorSetType, GLOBAL_UNIFORM_COUNT, IMAGE_TEXTURE_COUNT, LAYER_STORAGE_COUNT,
    LAYER_UNIFORM_COUNT, drawable_uniform_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::shaders::shader_program::ShaderProgram;
use crate::texture2d::Texture2D;
use crate::uniform_buffer::UniformBuffer;

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

/// A descriptor set for each frame in flight.
struct DescriptorSet {
    context: Weak<Context>,
    type_: DescriptorSetType,
    descriptor_sets: Vec<vk::DescriptorSet>,
    descriptor_pool_index: usize,
    /// The frame each frame's set was last bound in (`Context::frame_serial`).
    bound_serial: Vec<u64>,
}

impl DescriptorSet {
    fn new(context: &Context, type_: DescriptorSetType) -> Self {
        DescriptorSet {
            context: context.weak(),
            type_,
            descriptor_sets: Vec::new(),
            descriptor_pool_index: 0,
            bound_serial: Vec::new(),
        }
    }

    /// Allocates the sets if needed. Returns whether they are there.
    fn allocate(&mut self, context: &Context) -> bool {
        if !self.descriptor_sets.is_empty() {
            return true;
        }
        let Some((sets, pool_index)) = context.allocate_descriptor_sets(self.type_) else {
            return false;
        };
        self.bound_serial = vec![0; sets.len()];
        self.descriptor_sets = sets;
        self.descriptor_pool_index = pool_index;
        true
    }

    /// Gives the sets back to their pool once the frames using them are
    /// done.
    fn release(&mut self) {
        let sets = std::mem::take(&mut self.descriptor_sets);
        if sets.is_empty() {
            return;
        }
        let (type_, pool_index) = (self.type_, self.descriptor_pool_index);
        if let Some(context) = self.context.upgrade() {
            context
                .enqueue_deletion(Box::new(move |context| context.give_back_descriptor_sets(type_, pool_index, sets)));
        }
    }

    /// The set of the frame being recorded, to write `changed` descriptors
    /// to: a new one if the frame has bound the current one.
    fn writable(&mut self, context: &Context, changed: bool) -> Option<(usize, vk::DescriptorSet, bool)> {
        let frame = usize::from(context.get_current_frame_resource_index());
        let mut replaced = false;
        if changed && self.bound_serial.get(frame) == Some(&context.frame_serial()) {
            self.release();
            replaced = true;
        }
        if !self.allocate(context) {
            return None;
        }
        Some((frame, *self.descriptor_sets.get(frame)?, replaced))
    }

    /// `bind(encoder)`
    fn bind(&mut self, context: &Context, command_buffer: vk::CommandBuffer, frame: usize) {
        let Some(&set) = self.descriptor_sets.get(frame) else {
            return;
        };
        // SAFETY: `command_buffer` is recording, and the set has the layout
        // of the general pipeline layout's set of this type.
        unsafe {
            context.get_backend().get_device().cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                context.get_general_pipeline_layout(),
                self.type_.set_index(),
                &[set],
                &[],
            );
        }
        self.bound_serial[frame] = context.frame_serial();
    }
}

impl Drop for DescriptorSet {
    fn drop(&mut self) {
        self.release();
    }
}

/// What a buffer descriptor points at. The id tells a buffer from an earlier
/// one with the same handle (`buffer_resource::unique_id`).
#[derive(Clone, Copy, PartialEq, Eq)]
struct BufferBinding {
    id: u64,
    buffer: vk::Buffer,
    offset: vk::DeviceSize,
    range: vk::DeviceSize,
}

const NO_BUFFER: BufferBinding = BufferBinding { id: u64::MAX, buffer: vk::Buffer::null(), offset: 0, range: 0 };

/// The descriptor sets of a uniform buffer array.
pub(crate) struct UniformDescriptorSet {
    set: DescriptorSet,
    /// What each frame's set holds, by binding.
    written: Vec<[BufferBinding; MAX_BUFFER_BINDINGS]>,
}

impl UniformDescriptorSet {
    pub(crate) fn new(context: &Context, type_: DescriptorSetType) -> Self {
        UniformDescriptorSet { set: DescriptorSet::new(context, type_), written: Vec::new() }
    }

    /// The binding of the buffer `id` in a set of `type_`, by its kind.
    fn binding_of(type_: DescriptorSetType, id: usize, storage: bool) -> Option<u32> {
        match (type_, storage) {
            (DescriptorSetType::Global, false) => global_uniform_binding(id),
            (DescriptorSetType::Layer, true) => layer_storage_binding(id),
            (DescriptorSetType::Layer, false) => layer_uniform_binding(id),
            (DescriptorSetType::DrawableUniform, false) => drawable_uniform_binding(id),
            _ => None,
        }
    }

    /// Points the frame's set at the buffers of `uniforms`, each brought up to
    /// date with the frame before, and binds it. A binding without a buffer
    /// gets the context's placeholder.
    pub(crate) fn bind(&mut self, context: &Context, command_buffer: vk::CommandBuffer, uniforms: &UniformBufferArray) {
        let type_ = self.set.type_;
        let shape = type_.buffer_shape();
        let count = shape.storage + shape.uniform;
        let dummy = BufferBinding { id: 0, buffer: context.get_dummy_buffer(), offset: 0, range: vk::WHOLE_SIZE };
        let mut bindings = [dummy; MAX_BUFFER_BINDINGS];

        let current_index = context.get_current_frame_resource_index();
        let first_id = if type_ == DescriptorSetType::Global { 0 } else { LAYER_SSBO_START_ID };
        for id in first_id..uniforms.allocated_size() {
            let Some(uniform_buffer) = uniforms.get(id).and_then(|b| b.downcast_ref::<UniformBuffer>()) else {
                continue;
            };
            let Some(binding) = Self::binding_of(type_, id, uniform_buffer.is_storage()) else {
                debug_assert!(false, "no binding for the buffer {id} in the {type_:?} set");
                continue;
            };
            let resource = uniform_buffer.get_buffer_resource();
            if !resource.is_valid() {
                continue;
            }
            resource.update_vulkan_buffer(current_index);
            bindings[binding as usize] = BufferBinding {
                id: resource.allocation_id(),
                buffer: resource.get_vulkan_buffer(),
                offset: resource.get_vulkan_buffer_offset() as vk::DeviceSize,
                range: resource.get_size_in_bytes().max(1) as vk::DeviceSize,
            };
        }

        let frame = usize::from(current_index);
        let changed = self.written.get(frame).is_none_or(|written| written[..count] != bindings[..count]);
        let Some((frame, set, replaced)) = self.set.writable(context, changed) else {
            return;
        };
        if replaced || self.written.len() != self.set.descriptor_sets.len() {
            self.written = vec![[NO_BUFFER; MAX_BUFFER_BINDINGS]; self.set.descriptor_sets.len()];
        }
        if changed {
            let written = &mut self.written[frame];
            let buffer_infos: Vec<(u32, [vk::DescriptorBufferInfo; 1])> = (0..count)
                .filter(|&binding| written[binding] != bindings[binding])
                .map(|binding| {
                    let b = bindings[binding];
                    (binding as u32, [vk::DescriptorBufferInfo { buffer: b.buffer, offset: b.offset, range: b.range }])
                })
                .collect();
            let writes: Vec<vk::WriteDescriptorSet<'_>> = buffer_infos
                .iter()
                .map(|(binding, info)| {
                    let descriptor_type = if (*binding as usize) < shape.storage {
                        vk::DescriptorType::STORAGE_BUFFER
                    } else {
                        vk::DescriptorType::UNIFORM_BUFFER
                    };
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(*binding)
                        .descriptor_type(descriptor_type)
                        .buffer_info(info)
                })
                .collect();
            // SAFETY: the set isn't used by a pending command buffer (it is
            // this frame's, and not yet bound in it), and the buffers are
            // live with the usage of their descriptor type.
            unsafe { context.get_backend().get_device().update_descriptor_sets(&writes, &[]) };
            written[..count].copy_from_slice(&bindings[..count]);
        }
        self.set.bind(context, command_buffer, frame);
    }
}

/// What a texture's descriptors point at. The ids tell an image and a
/// sampler from earlier ones with the same handles
/// (`buffer_resource::unique_id`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageBinding {
    pub(crate) image_id: u64,
    pub(crate) sampler_id: u64,
    pub(crate) image_view: vk::ImageView,
    pub(crate) image_layout: vk::ImageLayout,
    pub(crate) sampler: vk::Sampler,
}

const NO_IMAGE: ImageBinding = ImageBinding {
    image_id: u64::MAX,
    sampler_id: u64::MAX,
    image_view: vk::ImageView::null(),
    image_layout: vk::ImageLayout::UNDEFINED,
    sampler: vk::Sampler::null(),
};

/// The descriptor sets of a drawable's textures.
pub(crate) struct ImageDescriptorSet {
    set: DescriptorSet,
    /// What each frame's set holds, by the texture's index in the shader.
    written: Vec<[ImageBinding; IMAGE_TEXTURE_COUNT]>,
}

impl ImageDescriptorSet {
    pub(crate) fn new(context: &Context) -> Self {
        ImageDescriptorSet { set: DescriptorSet::new(context, DescriptorSetType::DrawableImage), written: Vec::new() }
    }

    /// `update(textures)` and `bind(encoder)`: points the frame's set at
    /// the textures `shader` samples, and binds it. An index without a
    /// texture gets the context's placeholder.
    pub(crate) fn bind(
        &mut self,
        context: &Context,
        command_buffer: vk::CommandBuffer,
        textures: &Textures,
        shader: &ShaderProgram,
    ) {
        let Some(dummy) = context.get_dummy_texture() else {
            return;
        };
        let mut bindings = [dummy; IMAGE_TEXTURE_COUNT];
        for (id, texture) in textures.iter().enumerate() {
            let Some(texture) = texture else { continue };
            let Some(index) = shader.get_sampler_location(id).filter(|&index| index < IMAGE_TEXTURE_COUNT) else {
                continue;
            };
            let mut texture = texture.borrow_mut();
            let Some(texture) = texture.downcast_mut::<Texture2D>() else {
                continue;
            };
            let image_view = texture.get_vulkan_image_view();
            let sampler = texture.get_vulkan_sampler();
            if image_view == vk::ImageView::null() || sampler == vk::Sampler::null() {
                continue;
            }
            let (image_id, sampler_id) = texture.get_ids();
            bindings[index] = ImageBinding {
                image_id,
                sampler_id,
                image_view,
                image_layout: texture.get_vulkan_image_layout(),
                sampler,
            };
        }

        let frame = usize::from(context.get_current_frame_resource_index());
        let changed = self.written.get(frame).is_none_or(|written| *written != bindings);
        let Some((frame, set, replaced)) = self.set.writable(context, changed) else {
            return;
        };
        if replaced || self.written.len() != self.set.descriptor_sets.len() {
            self.written = vec![[NO_IMAGE; IMAGE_TEXTURE_COUNT]; self.set.descriptor_sets.len()];
        }
        if changed {
            let written = &mut self.written[frame];
            let image_infos: Vec<(usize, [vk::DescriptorImageInfo; 1], [vk::DescriptorImageInfo; 1])> = (0
                ..IMAGE_TEXTURE_COUNT)
                .filter(|&index| written[index] != bindings[index])
                .map(|index| {
                    let b = bindings[index];
                    (
                        index,
                        [vk::DescriptorImageInfo {
                            sampler: vk::Sampler::null(),
                            image_view: b.image_view,
                            image_layout: b.image_layout,
                        }],
                        [vk::DescriptorImageInfo {
                            sampler: b.sampler,
                            image_view: vk::ImageView::null(),
                            image_layout: vk::ImageLayout::UNDEFINED,
                        }],
                    )
                })
                .collect();
            let mut writes = Vec::with_capacity(image_infos.len() * 2);
            for (index, image, sampler) in &image_infos {
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(image_texture_binding(*index))
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(image),
                );
                writes.push(
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(image_sampler_binding(*index))
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(sampler),
                );
            }
            // SAFETY: as in `UniformDescriptorSet::bind`; the views and
            // samplers are live.
            unsafe { context.get_backend().get_device().update_descriptor_sets(&writes, &[]) };
            *written = bindings;
        }
        self.set.bind(context, command_buffer, frame);
    }
}
