//! The programs of the `color-relief` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_COLOR_RELIEF_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_COLOR_RELIEF_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const ID_COLOR_RELIEF_EVALUATED_PROPS_UBO: usize = ID_COLOR_RELIEF_TILE_PROPS_UBO + 1;
pub const COLOR_RELIEF_LAYER_UBO_COUNT: usize = ID_COLOR_RELIEF_EVALUATED_PROPS_UBO + 1;
pub const ID_COLOR_RELIEF_IMAGE_TEXTURE: usize = 0;
pub const ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: usize = 1;
pub const ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: usize = 2;
pub const ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    attributes: &[
        (ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2),
    ],
    instance_attributes: &[],
    textures: &[
        ID_COLOR_RELIEF_IMAGE_TEXTURE,
        ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
        ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE,
    ],
    properties: &[],
};
