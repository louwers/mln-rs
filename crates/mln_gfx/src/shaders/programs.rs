//! The engine's programs: `CLIPPING_MASK`, `SKY`, `ATMOSPHERE`,
//! `GLOBE_DEPTH`, `LAYER_COMPOSITE` and `TILE_DEBUG`. Each backend has their
//! sources.

use crate::gfx_types::AttributeDataType::Short2;
use crate::shaders::program::Program;
use crate::shaders::shader_defines::DRAWABLE_RESERVED_UBO_COUNT;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE: usize = 0;

pub const ID_TILE_DEBUG_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const TILE_DEBUG_UBO_COUNT: usize = ID_TILE_DEBUG_PROPS_UBO + 1;
/// The corner of the quad that draws a segment: x is 0 at the segment's
/// start and 1 at its end, y picks its side.
pub const ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE: usize = 0;
/// Per instance, where the segment starts and ends, in tile units.
pub const ID_TILE_DEBUG_START_ATTRIBUTE: usize = 1;
pub const ID_TILE_DEBUG_END_ATTRIBUTE: usize = 2;

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
/// as segments with round ends, one instance of a quad each, extruded on
/// screen to the width of the overlay's uniform block
/// ([`tile_debug_ubo`](crate::shaders::tile_debug_ubo)).
pub static TILE_DEBUG: Program = Program {
    name: "TileDebugShader",
    attributes: &[(ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[(ID_TILE_DEBUG_START_ATTRIBUTE, Short2), (ID_TILE_DEBUG_END_ATTRIBUTE, Short2)],
    textures: &[],
    properties: &[],
};
