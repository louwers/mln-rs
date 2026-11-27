//! The circles' program.

use mln_engine::draw::ground::{GROUND_DEPTH, GROUND_HEIGHT};
use mln_engine::shaders::programs::POS;
use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    buffers: &[
        ProgramBuffer::storage::<CircleDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<CircleEvaluatedPropsUBO>("props", 0, Stages::Both),
    ],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        Attribute::new("color", Float4),
        Attribute::new("radius", Float2),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_opacity", Float2),
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[
        Property::new("color"),
        Property::new("radius"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("stroke_color"),
        Property::new("stroke_width"),
        Property::new("stroke_opacity"),
    ],
    sources: Sources::NONE.wgsl(&wgsl::CIRCLE).msl(&msl::CIRCLE).glsl(&glsl::CIRCLE).glsl_es(&glsl_es::CIRCLE),
};
