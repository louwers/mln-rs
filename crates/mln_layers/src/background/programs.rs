//! The programs of the `background` layer type: the buffers, attributes and
//! textures of its shaders, and their sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Sources, Stages};

use super::ubo::{BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub static BACKGROUND: Program = Program {
    name: "background",
    buffers: &[ProgramBuffer::uniform::<BackgroundPropsUBO>("props", 0)],
    attributes: &[Attribute::new("position", Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_SHADER)
        .msl(&msl::BACKGROUND_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_SHADER)
        .glsl(&glsl::BACKGROUND_SHADER),
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "background-pattern",
    buffers: &[
        ProgramBuffer::storage::<BackgroundPatternDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<BackgroundPatternPropsUBO>("props", 0),
    ],
    attributes: &[Attribute::new("position", Short2)],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image")],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_PATTERN_SHADER)
        .msl(&msl::BACKGROUND_PATTERN_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_PATTERN_SHADER)
        .glsl(&glsl::BACKGROUND_PATTERN_SHADER),
};
