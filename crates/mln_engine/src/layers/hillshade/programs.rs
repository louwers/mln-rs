//! The hillshades' programs.

use mln_gfx::shaders::program::{Program, ProgramBuffer, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{HillshadeDrawableUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};
use crate::render::terrain::ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO};
use crate::shaders::programs::{IMAGE, POS, TEXTURE_POS};

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    buffers: &[
        ProgramBuffer::storage::<HillshadeDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::storage::<HillshadeTilePropsUBO>("tilePropsVector", Stages::Fragment),
        ProgramBuffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 0, Stages::Fragment),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE)
        .msl(&msl::HILLSHADE)
        .glsl(&glsl::HILLSHADE)
        .glsl_es(&glsl_es::HILLSHADE),
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    buffers: &[
        ProgramBuffer::draw_uniform::<HillshadePrepareDrawableUBO>("drawable", Stages::Vertex),
        ProgramBuffer::draw_uniform::<HillshadePrepareTilePropsUBO>("tileProps", Stages::Both),
    ],
    paint_params: None,
    projection: Projection::None,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE_PREPARE)
        .msl(&msl::HILLSHADE_PREPARE)
        .glsl(&glsl::HILLSHADE_PREPARE)
        .glsl_es(&glsl_es::HILLSHADE_PREPARE),
};
