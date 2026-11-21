//! The programs of the `background` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_BACKGROUND_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_BACKGROUND_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const BACKGROUND_UBO_COUNT: usize = ID_BACKGROUND_PROPS_UBO + 1;
pub const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_BACKGROUND_IMAGE_TEXTURE],
    properties: &[],
};
