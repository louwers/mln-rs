//! The programs of the `heatmap` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
};

use crate::shaders::property;

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    attributes: &[
        (ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        property("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    attributes: &[(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_COLOR_RAMP_TEXTURE],
    properties: &[],
};
