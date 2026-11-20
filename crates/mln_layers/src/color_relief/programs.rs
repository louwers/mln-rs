//! The programs of the `color-relief` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, ID_COLOR_RELIEF_IMAGE_TEXTURE,
    ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE,
};

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
