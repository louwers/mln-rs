//! The color reliefs' program.

use mln_gfx::shaders::program::{Program, ProgramBuffer, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};
use crate::shaders::programs::{IMAGE, POS, TEXTURE_POS};

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    buffers: &[
        ProgramBuffer::storage::<ColorReliefDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::storage::<ColorReliefTilePropsUBO>("tilePropsVector", Stages::Both),
        ProgramBuffer::uniform::<ColorReliefEvaluatedPropsUBO>("props", 1, Stages::Fragment),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[
        IMAGE,
        ProgramTexture::unfilterable("elevation_stops", Stages::Fragment),
        ProgramTexture::new("color_stops", Stages::Fragment),
    ],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLOR_RELIEF)
        .msl(&msl::COLOR_RELIEF)
        .glsl(&glsl::COLOR_RELIEF)
        .glsl_es(&glsl_es::COLOR_RELIEF),
};
