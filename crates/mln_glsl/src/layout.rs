// The descriptor sets of every pipeline, and the binding of each buffer and
// texture in them, for the ids in the ranges of
// mln_gfx::shaders::bindings (see the crate's doc comment).
//
// - A uniform block of a slot binds in the layer set at its id less one,
//   4 or 5: a program whose buffer of each draw's values that both stages
//   read takes id 5 has its uniform block at slot 1, id 6.
// - A drawable's uniform buffers are the drawable set, with the vertex
//   buffers that the pulled shaders read as storage buffers after them, at
//   the ids after the uniform buffers'.
// - A texture and its sampler are two descriptors, the sampler
//   `MAX_TEXTURES` bindings after the texture: naga's GLSL
//   front end has separate textures and samplers only.
// - The global set has no platform parameters: swapchains have the identity
//   transform.

use mln_gfx::shaders::bindings::{
    FIRST_PROGRAM_BUFFER_ID, FIRST_UNIFORM_ID, FIRST_VERTEX_BUFFER_ID, MAX_TEXTURES, MAX_VERTEX_BUFFERS,
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

    /// The set's index, at which `compose` declares the GLSL's buffers and
    /// textures.
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
pub const GLOBAL_UNIFORM_COUNT: usize = FIRST_PROGRAM_BUFFER_ID;

/// The storage buffers of the layer set: the bindings from 0, for the ids
/// from `FIRST_PROGRAM_BUFFER_ID` to `FIRST_UNIFORM_ID`. That last id is a
/// layer's first uniform buffer or its tile properties storage buffer, so
/// the set has a binding of each kind for it.
pub const LAYER_STORAGE_COUNT: usize = FIRST_UNIFORM_ID + 1 - FIRST_PROGRAM_BUFFER_ID;
/// The uniform buffers of the layer set: the bindings from
/// `LAYER_STORAGE_COUNT`.
pub const LAYER_UNIFORM_COUNT: usize = FIRST_VERTEX_BUFFER_ID - FIRST_UNIFORM_ID;

/// The uniform buffers of the drawable set: the bindings from 0.
pub const DRAWABLE_UNIFORM_COUNT: usize = FIRST_VERTEX_BUFFER_ID - FIRST_PROGRAM_BUFFER_ID;

/// The vertex buffers of the drawable set, which the pulled shaders read as
/// storage buffers: the bindings after its uniform buffers, for the ids
/// after `FIRST_VERTEX_BUFFER_ID`.
pub const DRAWABLE_VERTEX_COUNT: usize = MAX_VERTEX_BUFFERS;

/// The textures of the image set: the bindings from 0.
pub const IMAGE_TEXTURE_COUNT: usize = MAX_TEXTURES;
/// The binding of the first sampler of the image set, the sampler of the
/// texture at slot 0.
pub const IMAGE_SAMPLER_START: usize = MAX_TEXTURES;

/// The binding in the global set of the uniform buffer `id`.
#[must_use]
pub const fn global_uniform_binding(id: usize) -> Option<u32> {
    if id < GLOBAL_UNIFORM_COUNT { Some(id as u32) } else { None }
}

/// The binding in the layer set of the storage buffer `id`.
#[must_use]
pub const fn layer_storage_binding(id: usize) -> Option<u32> {
    if id >= FIRST_PROGRAM_BUFFER_ID && id < FIRST_PROGRAM_BUFFER_ID + LAYER_STORAGE_COUNT {
        Some((id - FIRST_PROGRAM_BUFFER_ID) as u32)
    } else {
        None
    }
}

/// The binding in the layer set of the uniform buffer `id`.
#[must_use]
pub const fn layer_uniform_binding(id: usize) -> Option<u32> {
    if id >= FIRST_UNIFORM_ID && id < FIRST_VERTEX_BUFFER_ID {
        Some((LAYER_STORAGE_COUNT + id - FIRST_UNIFORM_ID) as u32)
    } else {
        None
    }
}

/// The binding in the drawable set of the uniform buffer `id`.
#[must_use]
pub const fn drawable_uniform_binding(id: usize) -> Option<u32> {
    if id >= FIRST_PROGRAM_BUFFER_ID && id < FIRST_VERTEX_BUFFER_ID {
        Some((id - FIRST_PROGRAM_BUFFER_ID) as u32)
    } else {
        None
    }
}

/// The binding in the drawable set of the vertex buffer `id`, which a shader
/// reads as a storage buffer.
#[must_use]
pub const fn drawable_vertex_binding(id: usize) -> Option<u32> {
    if id >= FIRST_VERTEX_BUFFER_ID && id < FIRST_VERTEX_BUFFER_ID + DRAWABLE_VERTEX_COUNT {
        Some((id - FIRST_PROGRAM_BUFFER_ID) as u32)
    } else {
        None
    }
}

/// The binding in the image set of the texture at slot `index` of its
/// program's textures.
#[must_use]
pub const fn image_texture_binding(index: usize) -> u32 {
    index as u32
}

/// The binding in the image set of the sampler of the texture at `index`.
#[must_use]
pub const fn image_sampler_binding(index: usize) -> u32 {
    (IMAGE_SAMPLER_START + index) as u32
}

/// The size of the push constants of the general pipeline layout: the
/// drawable's index into its layer's consolidated buffers (`int drawIndex`).
pub const DRAW_INDEX_PUSH_CONSTANT_SIZE: u32 = size_of::<u32>() as u32;
