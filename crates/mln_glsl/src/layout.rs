// The descriptor sets of every pipeline, and the binding of each buffer and
// texture in them, for the packed ids of mln_gfx::shaders::shader_defines
// (see the crate's doc comment).
//
// - A layer's uniform buffer binds at its id less one: the first one of a
//   layer has the id 5, or 6 for the layers with a tile properties storage
//   buffer at 5 (fill, fill extrusion, color relief), and binds at 4 or 5.
// - A drawable's uniform buffers are the drawable set, with the vertex
//   buffers that the pulled shaders read as storage buffers after them, at
//   the ids after the uniform buffers'.
// - A texture and its sampler are two descriptors, the sampler
//   `MAX_TEXTURE_COUNT_PER_SHADER` bindings after the texture: naga's GLSL
//   front end has separate textures and samplers only.
// - The global set has no platform parameters: swapchains have the identity
//   transform.

use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, GLOBAL_UBO_COUNT, LAYER_SSBO_START_ID, LAYER_UBO_START_ID,
    MAX_TEXTURE_COUNT_PER_SHADER, MAX_UBO_COUNT_PER_SHADER,
};

/// The descriptor sets of a pipeline layout, by set index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DescriptorSetType {
    Global,
    Layer,
    DrawableUniform,
    DrawableImage,
}

impl DescriptorSetType {
    pub const COUNT: usize = 4;
    pub const ALL: [DescriptorSetType; Self::COUNT] = [
        DescriptorSetType::Global,
        DescriptorSetType::Layer,
        DescriptorSetType::DrawableUniform,
        DescriptorSetType::DrawableImage,
    ];

    /// `GLOBAL_SET_INDEX`, `LAYER_SET_INDEX`, `DRAWABLE_UBO_SET_INDEX` and
    /// `DRAWABLE_IMAGE_SET_INDEX` of the GLSL.
    #[must_use]
    pub const fn set_index(self) -> u32 {
        self as u32
    }

    /// The buffers of the type's sets; none for the image set.
    #[must_use]
    pub const fn buffer_shape(self) -> BufferSetShape {
        match self {
            DescriptorSetType::Global => BufferSetShape { storage: 0, uniform: GLOBAL_UNIFORM_COUNT, vertex: 0 },
            DescriptorSetType::Layer => {
                BufferSetShape { storage: LAYER_STORAGE_COUNT, uniform: LAYER_UNIFORM_COUNT, vertex: 0 }
            }
            DescriptorSetType::DrawableUniform => {
                BufferSetShape { storage: 0, uniform: DRAWABLE_UNIFORM_COUNT, vertex: DRAWABLE_VERTEX_COUNT }
            }
            DescriptorSetType::DrawableImage => BufferSetShape { storage: 0, uniform: 0, vertex: 0 },
        }
    }
}

/// The descriptors of a buffer set: `storage` storage buffers from binding
/// 0, then `uniform` uniform buffers, then `vertex` storage buffers that bind
/// vertex buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSetShape {
    pub storage: usize,
    pub uniform: usize,
    pub vertex: usize,
}

impl BufferSetShape {
    /// How many bindings the set has.
    #[must_use]
    pub const fn len(self) -> usize {
        self.storage + self.uniform + self.vertex
    }

    /// Whether the set has no bindings.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Whether `binding` is a storage buffer's, rather than a uniform
    /// buffer's.
    #[must_use]
    pub const fn is_storage(self, binding: usize) -> bool {
        binding < self.storage || binding >= self.storage + self.uniform
    }
}

/// The uniform buffers of the global set, bound at their ids.
pub const GLOBAL_UNIFORM_COUNT: usize = GLOBAL_UBO_COUNT;

/// The storage buffers of the layer set: the bindings from 0
/// (`layerSSBOStartId` in the GLSL).
pub const LAYER_STORAGE_COUNT: usize = LAYER_UBO_START_ID - LAYER_SSBO_START_ID;
/// The uniform buffers of the layer set: the bindings from
/// `LAYER_STORAGE_COUNT` (`layerUBOStartId` in the GLSL).
pub const LAYER_UNIFORM_COUNT: usize = MAX_UBO_COUNT_PER_SHADER - DRAWABLE_RESERVED_UBO_COUNT;

/// The uniform buffers of the drawable set: the bindings from 0
/// (`drawableUBOStartId` in the GLSL).
pub const DRAWABLE_UNIFORM_COUNT: usize = MAX_UBO_COUNT_PER_SHADER - LAYER_SSBO_START_ID;

/// The vertex buffers of the drawable set, which the pulled shaders read as
/// storage buffers: the bindings after its uniform buffers, for the ids
/// after `MAX_UBO_COUNT_PER_SHADER`.
pub const DRAWABLE_VERTEX_COUNT: usize = 3;

/// The textures of the image set: the bindings from 0.
pub const IMAGE_TEXTURE_COUNT: usize = MAX_TEXTURE_COUNT_PER_SHADER;
/// The binding of the first sampler of the image set
/// (`samplerBindingStartId` in the GLSL).
pub const IMAGE_SAMPLER_START: usize = MAX_TEXTURE_COUNT_PER_SHADER;

/// The binding in the global set of the uniform buffer `id`.
#[must_use]
pub const fn global_uniform_binding(id: usize) -> Option<u32> {
    if id < GLOBAL_UNIFORM_COUNT { Some(id as u32) } else { None }
}

/// The binding in the layer set of the storage buffer `id`.
#[must_use]
pub const fn layer_storage_binding(id: usize) -> Option<u32> {
    if id >= LAYER_SSBO_START_ID && id < LAYER_UBO_START_ID { Some((id - LAYER_SSBO_START_ID) as u32) } else { None }
}

/// The binding in the layer set of the uniform buffer `id`.
#[must_use]
pub const fn layer_uniform_binding(id: usize) -> Option<u32> {
    if id >= DRAWABLE_RESERVED_UBO_COUNT && id < MAX_UBO_COUNT_PER_SHADER {
        Some((LAYER_STORAGE_COUNT + id - DRAWABLE_RESERVED_UBO_COUNT) as u32)
    } else {
        None
    }
}

/// The binding in the drawable set of the uniform buffer `id`.
#[must_use]
pub const fn drawable_uniform_binding(id: usize) -> Option<u32> {
    if id >= LAYER_SSBO_START_ID && id < MAX_UBO_COUNT_PER_SHADER {
        Some((id - LAYER_SSBO_START_ID) as u32)
    } else {
        None
    }
}

/// The binding in the drawable set of the vertex buffer `id`, which a shader
/// reads as a storage buffer.
#[must_use]
pub const fn drawable_vertex_binding(id: usize) -> Option<u32> {
    if id >= MAX_UBO_COUNT_PER_SHADER && id < MAX_UBO_COUNT_PER_SHADER + DRAWABLE_VERTEX_COUNT {
        Some((id - LAYER_SSBO_START_ID) as u32)
    } else {
        None
    }
}

/// The binding in the image set of the program's texture at `slot`.
#[must_use]
pub const fn image_texture_binding(slot: usize) -> u32 {
    slot as u32
}

/// The binding in the image set of the sampler of the program's texture at
/// `slot`.
#[must_use]
pub const fn image_sampler_binding(slot: usize) -> u32 {
    (IMAGE_SAMPLER_START + slot) as u32
}

/// The size of the push constants of the general pipeline layout: the
/// drawable's index into its layer's consolidated buffers (`int ubo_index`).
pub const UBO_INDEX_PUSH_CONSTANT_SIZE: u32 = size_of::<u32>() as u32;
