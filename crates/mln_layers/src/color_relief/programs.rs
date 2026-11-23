//! The programs of the `color-relief` layer type: the buffers, attributes
//! and textures of its shader, and its sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Sources, Stages};

use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

/// A tile's terrain, colored by elevation.
pub static COLOR_RELIEF: Program = Program {
    name: "color-relief",
    buffers: &[
        ProgramBuffer::storage::<ColorReliefDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::storage::<ColorReliefTilePropsUBO>("tilePropsVector", Stages::Both),
        ProgramBuffer::uniform::<ColorReliefEvaluatedPropsUBO>("props", 1),
    ],
    attributes: &[Attribute::new("pos", Short2), Attribute::new("texture_pos", Short2)],
    instance_attributes: &[],
    // The elevations' bytes, which the shader decodes, and the elevations
    // of the stops, which it compares them with.
    textures: &[
        ProgramTexture::new("image").precise(),
        ProgramTexture::unfilterable("elevation_stops").precise(),
        ProgramTexture::new("color_stops"),
    ],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::COLOR_RELIEF_SHADER)
        .msl(&msl::COLOR_RELIEF_SHADER)
        .glsl_es(&glsl_es::COLOR_RELIEF_SHADER)
        .glsl(&glsl::COLOR_RELIEF_SHADER),
};
