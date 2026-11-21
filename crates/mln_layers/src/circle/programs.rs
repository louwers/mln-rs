//! The programs of the `circle` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_CIRCLE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_CIRCLE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const CIRCLE_LAYER_UBO_COUNT: usize = ID_CIRCLE_EVALUATED_PROPS_UBO + 1;
pub const ID_CIRCLE_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub const ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE: usize = 7;

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
        Property::new("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        Property::new("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        Property::new("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        Property::new("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        Property::new("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
};
