//! The programs of the `hillshade` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_HILLSHADE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_HILLSHADE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO; // SSBO
pub const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const HILLSHADE_LAYER_UBO_COUNT: usize = ID_HILLSHADE_EVALUATED_PROPS_UBO + 1;
pub const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const ID_HILLSHADE_IMAGE_TEXTURE: usize = 0;
pub const ID_HILLSHADE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};
