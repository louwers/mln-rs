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
        Attribute { id: 0, format: Short2 },
        Attribute { id: 1, format: Float4 },
        Attribute { id: 2, format: Float2 },
        Attribute { id: 3, format: Float2 },
        Attribute { id: 4, format: Float2 },
        Attribute { id: 5, format: Float4 },
        Attribute { id: 6, format: Float2 },
        Attribute { id: 7, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::color("color", 1),
        Property::float("radius", 2),
        Property::float("blur", 3),
        Property::float("opacity", 4),
        Property::color("stroke_color", 5),
        Property::float("stroke_width", 6),
        Property::float("stroke_opacity", 7),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::CIRCLE_SHADER)
        .msl(&msl::CIRCLE_SHADER)
        .glsl_es(&glsl_es::CIRCLE_SHADER)
        .glsl(&glsl::CIRCLE_SHADER),
};
