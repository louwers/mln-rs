//! The programs of the `heatmap` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Texture};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_HEATMAP_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_HEATMAP_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_HEATMAP_TEXTURE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_HEATMAP_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_HEATMAP_COLOR_RAMP_TEXTURE: usize = 1;
pub(crate) const ID_HEATMAP_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub(crate) const ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE: usize = 2;

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    buffers: &[
        Buffer::storage::<HeatmapDrawableUBO>(ID_HEATMAP_DRAWABLE_UBO),
        Buffer::uniform::<HeatmapEvaluatedPropsUBO>(ID_HEATMAP_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, "a_weight", Float2),
        Attribute::new(ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, "a_radius", Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        Property::float("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(wgsl: HEATMAP_SHADER, msl: HEATMAP_SHADER, glsl_es: HEATMAP_SHADER, glsl: HEATMAP_SHADER,),
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    buffers: &[Buffer::uniform::<HeatmapTexturePropsUBO>(ID_HEATMAP_TEXTURE_PROPS_UBO)],
    attributes: &[Attribute::new(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, "a_pos", Short2)],
    instance_attributes: &[],
    textures: &[Texture::new("u_image"), Texture::new("u_color_ramp")],
    properties: &[],
    sources: sources!(
        wgsl: HEATMAP_TEXTURE_SHADER,
        msl: HEATMAP_TEXTURE_SHADER,
        glsl_es: HEATMAP_TEXTURE_SHADER,
        glsl: HEATMAP_TEXTURE_SHADER,
    ),
};
