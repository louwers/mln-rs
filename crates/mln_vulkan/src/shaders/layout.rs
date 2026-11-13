// The descriptor sets of every pipeline, and the binding of each buffer and
// texture in them. Ports the `DescriptorSetType` enum of
// include/mln/vulkan/descriptor_set.hpp and the set and binding macros of
// include/mln/shaders/vulkan/common.hpp, for the packed ids of
// mln_gfx::shaders::shader_defines (see the module comment of `shaders`).
//
// Differences from C++:
// - A layer's uniform buffer binds at its id less one: the ids 5 and 6 at
//   the bindings 4 and 5. In C++ the first uniform buffer of every layer has
//   the id that binds at 4; here it is 5 or, for the layers with a tile
//   properties storage buffer at 5 (fill, fill extrusion, color relief), 6.
// - A drawable's uniform buffers are the whole drawable set. C++ puts four
//   storage buffers before them, for the instance data of symbols and fill
//   extrusions; the port's symbols aren't instanced, and its fill extrusions
//   read their instances as vertex inputs.
// - A texture and its sampler are two descriptors, the sampler
//   `MAX_TEXTURE_COUNT_PER_SHADER` bindings after the texture: naga's GLSL
//   front end has Vulkan's separate textures and samplers only, where C++
//   declares combined ones.
// - The global set has no `GlobalPlatformParamsUBO`: the surface
//   pre-rotation it carries is Android's.

use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, GLOBAL_UBO_COUNT, LAYER_SSBO_START_ID, LAYER_UBO_START_ID,
    MAX_TEXTURE_COUNT_PER_SHADER, MAX_UBO_COUNT_PER_SHADER,
};

/// The descriptor sets of a pipeline layout, by set index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum DescriptorSetType {
    Global,
    Layer,
    DrawableUniform,
    DrawableImage,
}

impl DescriptorSetType {
    pub(crate) const COUNT: usize = 4;
    pub(crate) const ALL: [DescriptorSetType; Self::COUNT] = [
        DescriptorSetType::Global,
        DescriptorSetType::Layer,
        DescriptorSetType::DrawableUniform,
        DescriptorSetType::DrawableImage,
    ];

    /// `GLOBAL_SET_INDEX`, `LAYER_SET_INDEX`, `DRAWABLE_UBO_SET_INDEX` and
    /// `DRAWABLE_IMAGE_SET_INDEX` of the GLSL.
    #[must_use]
    pub(crate) const fn set_index(self) -> u32 {
        self as u32
    }
}

/// The uniform buffers of the global set, bound at their ids.
pub(crate) const GLOBAL_UNIFORM_COUNT: usize = GLOBAL_UBO_COUNT;

/// The storage buffers of the layer set: the bindings from 0
/// (`layerSSBOStartId` in the GLSL).
pub(crate) const LAYER_STORAGE_COUNT: usize = LAYER_UBO_START_ID - LAYER_SSBO_START_ID;
/// The uniform buffers of the layer set: the bindings from
/// `LAYER_STORAGE_COUNT` (`layerUBOStartId` in the GLSL).
pub(crate) const LAYER_UNIFORM_COUNT: usize = MAX_UBO_COUNT_PER_SHADER - DRAWABLE_RESERVED_UBO_COUNT;

/// The uniform buffers of the drawable set: the bindings from 0
/// (`drawableUBOStartId` in the GLSL).
pub(crate) const DRAWABLE_UNIFORM_COUNT: usize = MAX_UBO_COUNT_PER_SHADER - LAYER_SSBO_START_ID;

/// The textures of the image set: the bindings from 0.
pub(crate) const IMAGE_TEXTURE_COUNT: usize = MAX_TEXTURE_COUNT_PER_SHADER;
/// The binding of the first sampler of the image set
/// (`samplerBindingStartId` in the GLSL).
pub(crate) const IMAGE_SAMPLER_START: usize = MAX_TEXTURE_COUNT_PER_SHADER;

/// The binding in the global set of the uniform buffer `id`.
#[must_use]
pub(crate) const fn global_uniform_binding(id: usize) -> Option<u32> {
    if id < GLOBAL_UNIFORM_COUNT { Some(id as u32) } else { None }
}

/// The binding in the layer set of the storage buffer `id`.
#[must_use]
pub(crate) const fn layer_storage_binding(id: usize) -> Option<u32> {
    if id >= LAYER_SSBO_START_ID && id < LAYER_UBO_START_ID { Some((id - LAYER_SSBO_START_ID) as u32) } else { None }
}

/// The binding in the layer set of the uniform buffer `id`.
#[must_use]
pub(crate) const fn layer_uniform_binding(id: usize) -> Option<u32> {
    if id >= DRAWABLE_RESERVED_UBO_COUNT && id < MAX_UBO_COUNT_PER_SHADER {
        Some((LAYER_STORAGE_COUNT + id - DRAWABLE_RESERVED_UBO_COUNT) as u32)
    } else {
        None
    }
}

/// The binding in the drawable set of the uniform buffer `id`.
#[must_use]
pub(crate) const fn drawable_uniform_binding(id: usize) -> Option<u32> {
    if id >= LAYER_SSBO_START_ID && id < MAX_UBO_COUNT_PER_SHADER {
        Some((id - LAYER_SSBO_START_ID) as u32)
    } else {
        None
    }
}

/// The binding in the image set of the texture at `index`
/// (`TextureInfo::index`).
#[must_use]
pub(crate) const fn image_texture_binding(index: usize) -> u32 {
    index as u32
}

/// The binding in the image set of the sampler of the texture at `index`.
#[must_use]
pub(crate) const fn image_sampler_binding(index: usize) -> u32 {
    (IMAGE_SAMPLER_START + index) as u32
}

/// The size of the push constants of the general pipeline layout: the
/// drawable's index into its layer's consolidated buffers (`int ubo_index`).
pub(crate) const UBO_INDEX_PUSH_CONSTANT_SIZE: u32 = std::mem::size_of::<u32>() as u32;

/// The largest push-constant block, the globe clip mask's two matrices,
/// which is all the guaranteed 128 bytes hold.
pub(crate) const CLIP_MASK_PUSH_CONSTANT_SIZE: u32 = 2 * std::mem::size_of::<[f32; 16]>() as u32;
