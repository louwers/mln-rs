//! The programs of the `background` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE};

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
