//! The programs of the `heatmap` layer type: the buffers, attributes,
//! textures and paint properties of its shaders, and their sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Short2};
use mln_gfx::shaders::program::{
    Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Sources, Stages,
};

use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

/// The heat of a tile's points, which draws into the layer's target.
pub static HEATMAP: Program = Program {
    name: "heatmap",
    buffers: &[
        ProgramBuffer::storage::<HeatmapDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<HeatmapEvaluatedPropsUBO>("props", 0),
    ],
    attributes: &[Attribute::new("pos", Short2), Attribute::new("weight", Float2), Attribute::new("radius", Float2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::float("weight"), Property::float("radius")],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::HEATMAP_SHADER)
        .msl(&msl::HEATMAP_SHADER)
        .glsl_es(&glsl_es::HEATMAP_SHADER)
        .glsl(&glsl::HEATMAP_SHADER),
};

/// The target's heat, colored through the ramp onto the frame.
pub static HEATMAP_TEXTURE: Program = Program {
    name: "heatmap-texture",
    buffers: &[ProgramBuffer::uniform::<HeatmapTexturePropsUBO>("props", 0)],
    attributes: &[Attribute::new("pos", Short2)],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image"), ProgramTexture::new("color_ramp")],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::HEATMAP_TEXTURE_SHADER)
        .msl(&msl::HEATMAP_TEXTURE_SHADER)
        .glsl_es(&glsl_es::HEATMAP_TEXTURE_SHADER)
        .glsl(&glsl::HEATMAP_TEXTURE_SHADER),
};
