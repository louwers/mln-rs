//! The backgrounds' programs.

use mln_gfx::shaders::program::{Program, ProgramBuffer, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};
use crate::shaders::programs::{IMAGE, POS};

/// The background's buffers: a background and a background pattern draw
/// write one buffer of draws.
const BACKGROUND_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>("drawableVector", Stages::Vertex),
    ProgramBuffer::uniform::<BackgroundPropsUBO>("props", 0, Stages::Fragment),
];

const BACKGROUND_PATTERN_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(
        "drawableVector",
        Stages::Vertex,
    ),
    ProgramBuffer::uniform::<BackgroundPatternPropsUBO>("props", 0, Stages::Both),
];

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    buffers: BACKGROUND_BUFFERS,
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND)
        .msl(&msl::BACKGROUND)
        .glsl(&glsl::BACKGROUND)
        .glsl_es(&glsl_es::BACKGROUND),
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    buffers: BACKGROUND_PATTERN_BUFFERS,
    paint_params: Some(Stages::Fragment),
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_PATTERN)
        .msl(&msl::BACKGROUND_PATTERN)
        .glsl(&glsl::BACKGROUND_PATTERN)
        .glsl_es(&glsl_es::BACKGROUND_PATTERN),
};
