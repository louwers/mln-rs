//! The heatmaps' programs.

use mln_gfx::gfx_types::AttributeDataType::Float2;
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};
use crate::render::terrain::programs::GROUND_HEIGHT;
use crate::shaders::programs::{IMAGE, POS};

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    buffers: &[
        ProgramBuffer::storage::<HeatmapDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<HeatmapEvaluatedPropsUBO>("props", 0, Stages::Both),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS, Attribute::new("weight", Float2), Attribute::new("radius", Float2), GROUND_HEIGHT],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::new("weight"), Property::new("radius")],
    sources: Sources::NONE.wgsl(&wgsl::HEATMAP).msl(&msl::HEATMAP).glsl(&glsl::HEATMAP).glsl_es(&glsl_es::HEATMAP),
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    buffers: &[ProgramBuffer::uniform::<HeatmapTexturePropsUBO>("ubo", 0, Stages::Both)],
    paint_params: Some(Stages::Vertex),
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("color_ramp", Stages::Fragment)],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HEATMAP_TEXTURE)
        .msl(&msl::HEATMAP_TEXTURE)
        .glsl(&glsl::HEATMAP_TEXTURE)
        .glsl_es(&glsl_es::HEATMAP_TEXTURE),
};
