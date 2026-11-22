//! The programs of the `background` layer type: the buffers, attributes and
//! textures of its shaders, and their sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Sources, Stages, Texture};

use super::ubo::{BackgroundDrawableUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub static BACKGROUND: Program = Program {
    name: "background",
    buffers: &[
        Buffer::storage::<BackgroundDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::uniform::<BackgroundPropsUBO>("props", 0),
    ],
    attributes: &[Attribute { id: 0, format: Short2 }],
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
        Buffer::storage::<BackgroundPatternDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::uniform::<BackgroundPatternPropsUBO>("props", 0),
    ],
    attributes: &[Attribute { id: 0, format: Short2 }],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_PATTERN_SHADER)
        .msl(&msl::BACKGROUND_PATTERN_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_PATTERN_SHADER)
        .glsl(&glsl::BACKGROUND_PATTERN_SHADER),
};
