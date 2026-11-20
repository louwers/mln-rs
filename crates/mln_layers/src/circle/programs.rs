//! The programs of the `circle` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE,
    ID_CIRCLE_POS_VERTEX_ATTRIBUTE, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE,
    ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE,
};

use crate::shaders::property;

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    attributes: &[
        (ID_CIRCLE_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        property("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        property("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        property("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        property("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        property("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
};
