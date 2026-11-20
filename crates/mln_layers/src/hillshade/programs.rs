//! The programs of the `hillshade` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_HILLSHADE_IMAGE_TEXTURE, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE,
};

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
