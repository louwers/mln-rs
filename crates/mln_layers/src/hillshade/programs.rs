//! The programs of the `hillshade` layer type: the buffers, attributes and
//! textures of its shaders, and their sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Sources, Stages, Texture};

use super::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

/// A tile's terrain, shaded from its slopes.
pub static HILLSHADE: Program = Program {
    name: "hillshade",
    buffers: &[
        Buffer::storage::<HillshadeDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::storage::<HillshadeTilePropsUBO>("tilePropsVector", Stages::Fragment),
        Buffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 0),
    ],
    attributes: &[Attribute::new("pos", Short2), Attribute::new("texture_pos", Short2)],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE_SHADER)
        .msl(&msl::HILLSHADE_SHADER)
        .glsl_es(&glsl_es::HILLSHADE_SHADER)
        .glsl(&glsl::HILLSHADE_SHADER),
};

/// A tile's slopes, prepared from its elevations into a target of its own.
pub static HILLSHADE_PREPARE: Program = Program {
    name: "hillshade-prepare",
    buffers: &[
        Buffer::draw_uniform::<HillshadePrepareDrawableUBO>("drawable", Stages::Vertex),
        Buffer::draw_uniform::<HillshadePrepareTilePropsUBO>("tileProps", Stages::Both),
    ],
    attributes: &[Attribute::new("pos", Short2), Attribute::new("texture_pos", Short2)],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE_PREPARE_SHADER)
        .msl(&msl::HILLSHADE_PREPARE_SHADER)
        .glsl_es(&glsl_es::HILLSHADE_PREPARE_SHADER)
        .glsl(&glsl::HILLSHADE_PREPARE_SHADER),
};
