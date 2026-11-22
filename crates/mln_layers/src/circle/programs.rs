//! The programs of the `circle` layer type: the buffers, attributes and
//! paint properties of its shaders, and their sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages};

use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub static CIRCLE: Program = Program {
    name: "circle",
    buffers: &[
        Buffer::storage::<CircleDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::uniform::<CircleEvaluatedPropsUBO>("props", 0),
    ],
    attributes: &[
        Attribute::new("pos", Short2),
        Attribute::new("color", Float4),
        Attribute::new("radius", Float2),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_opacity", Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::color("color"),
        Property::float("radius"),
        Property::float("blur"),
        Property::float("opacity"),
        Property::color("stroke_color"),
        Property::float("stroke_width"),
        Property::float("stroke_opacity"),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::CIRCLE_SHADER)
        .msl(&msl::CIRCLE_SHADER)
        .glsl_es(&glsl_es::CIRCLE_SHADER)
        .glsl(&glsl::CIRCLE_SHADER),
};
