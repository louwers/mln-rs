//! The programs of the `circle` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_CIRCLE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_CIRCLE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_CIRCLE_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub(crate) const ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE: usize = 2;
pub(crate) const ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub(crate) const ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub(crate) const ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE: usize = 6;
pub(crate) const ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE: usize = 7;

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    buffers: &[
        Buffer::storage::<CircleDrawableUBO>(ID_CIRCLE_DRAWABLE_UBO),
        Buffer::uniform::<CircleEvaluatedPropsUBO>(ID_CIRCLE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_CIRCLE_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, "a_radius", Float2),
        Attribute::new(ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE, "a_stroke_color", Float4),
        Attribute::new(ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE, "a_stroke_width", Float2),
        Attribute::new(ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, "a_stroke_opacity", Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        Property::new("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        Property::new("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        Property::new("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(wgsl: CIRCLE_SHADER, msl: CIRCLE_SHADER, glsl_es: CIRCLE_SHADER, glsl: CIRCLE_SHADER,),
};
