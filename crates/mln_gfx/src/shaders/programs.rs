//! The engine's programs: `CLIPPING_MASK`, `SKY`, `ATMOSPHERE`,
//! `GLOBE_DEPTH`, `LAYER_COMPOSITE` and `TILE_DEBUG`. Each backend has their
//! sources.

use crate::gfx_types::AttributeDataType::{Short2, UByte4};
use crate::shaders::program::Program;
use crate::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_TILE_DEBUG_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_TILE_DEBUG_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const TILE_DEBUG_UBO_COUNT: usize = ID_TILE_DEBUG_PROPS_UBO + 1;
pub const ID_TILE_DEBUG_POS_NORMAL_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_TILE_DEBUG_DATA_VERTEX_ATTRIBUTE: usize = 1;

pub const ID_LAYER_COMPOSITE_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const LAYER_COMPOSITE_UBO_COUNT: usize = ID_LAYER_COMPOSITE_UBO + 1;
pub const ID_LAYER_COMPOSITE_IMAGE_TEXTURE: usize = 0;
pub const ID_LAYER_COMPOSITE_BACKDROP_TEXTURE: usize = 1;
pub const ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_SKY_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const SKY_LAYER_UBO_COUNT: usize = ID_SKY_PROPS_UBO + 1;
pub const ID_ATMOSPHERE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ATMOSPHERE_LAYER_UBO_COUNT: usize = ID_ATMOSPHERE_PROPS_UBO + 1;
pub const ID_SKY_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    attributes: &[(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static GLOBE_DEPTH: Program = Program {
    name: "GlobeDepthShader",
    attributes: &[(ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static SKY: Program = Program {
    name: "SkyShader",
    attributes: &[(ID_SKY_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static ATMOSPHERE: Program = Program {
    name: "AtmosphereShader",
    attributes: &[(ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    attributes: &[(ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE],
    properties: &[],
};

/// The engine's debug overlays of tiles: their borders and their text, drawn
/// as lines whose color, width and other properties are uniforms
/// ([`tile_debug_ubo`](crate::shaders::tile_debug_ubo)).
pub static TILE_DEBUG: Program = Program {
    name: "TileDebugShader",
    attributes: &[(ID_TILE_DEBUG_POS_NORMAL_VERTEX_ATTRIBUTE, Short2), (ID_TILE_DEBUG_DATA_VERTEX_ATTRIBUTE, UByte4)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};
