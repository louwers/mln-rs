//! The programs of the `circle` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Sources, Stages};

use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_CIRCLE_DRAWABLE_UBO: usize = DRAWABLE.id;
pub(crate) const ID_CIRCLE_EVALUATED_PROPS_UBO: usize = PROPS.id;
pub(crate) const ID_CIRCLE_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub(crate) const ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE: usize = 2;
pub(crate) const ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub(crate) const ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub(crate) const ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE: usize = 6;
pub(crate) const ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE: usize = 7;

const DRAWABLE: Buffer = Buffer::storage::<CircleDrawableUBO>("drawableVector", Stages::Vertex);
const PROPS: Buffer = Buffer::uniform::<CircleEvaluatedPropsUBO>("props", 0);

pub static CIRCLE: Program = Program {
    name: "circle",
    buffers: &[
        DRAWABLE,
        PROPS,
    ],
    attributes: &[
        Attribute { id: ID_CIRCLE_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::color("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        Property::float("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        Property::float("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        Property::color("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        Property::float("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::CIRCLE_SHADER)
        .msl(&msl::CIRCLE_SHADER)
        .glsl_es(&glsl_es::CIRCLE_SHADER)
        .glsl(&glsl::CIRCLE_SHADER),
};
