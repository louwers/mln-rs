//! The engine's programs: `CLIPPING_MASK`, `SKY`, `ATMOSPHERE`,
//! `GLOBE_DEPTH`, `LAYER_COMPOSITE` and `TILE_DEBUG`. Each backend has their
//! sources.

use crate::gfx_types::AttributeDataType::{Short2, UByte4};
use crate::shaders::program::Program;
use crate::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE,
};

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
    attributes: &[(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2), (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};
