//! The programs of the `heatmap` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Sources, Stages, Texture};

use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_HEATMAP_DRAWABLE_UBO: usize = DRAWABLE.id;
pub(crate) const ID_HEATMAP_EVALUATED_PROPS_UBO: usize = PROPS.id;
pub(crate) const ID_HEATMAP_TEXTURE_PROPS_UBO: usize = TEXTURE_PROPS.id;
pub(crate) const ID_HEATMAP_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_HEATMAP_COLOR_RAMP_TEXTURE: usize = 1;
pub(crate) const ID_HEATMAP_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub(crate) const ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE: usize = 2;

const DRAWABLE: Buffer = Buffer::storage::<HeatmapDrawableUBO>("drawableVector", Stages::Vertex);
const PROPS: Buffer = Buffer::uniform::<HeatmapEvaluatedPropsUBO>("props", 0);
const TEXTURE_PROPS: Buffer = Buffer::uniform::<HeatmapTexturePropsUBO>("props", 0);

pub static HEATMAP: Program = Program {
    name: "heatmap",
    buffers: &[DRAWABLE, PROPS],
    attributes: &[
        Attribute { id: ID_HEATMAP_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        Property::float("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::HEATMAP_SHADER)
        .msl(&msl::HEATMAP_SHADER)
        .glsl_es(&glsl_es::HEATMAP_SHADER)
        .glsl(&glsl::HEATMAP_SHADER),
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "heatmap-texture",
    buffers: &[TEXTURE_PROPS],
    attributes: &[Attribute { id: ID_HEATMAP_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[Texture::new("u_image"), Texture::new("u_color_ramp")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HEATMAP_TEXTURE_SHADER)
        .msl(&msl::HEATMAP_TEXTURE_SHADER)
        .glsl_es(&glsl_es::HEATMAP_TEXTURE_SHADER)
        .glsl(&glsl::HEATMAP_TEXTURE_SHADER),
};
