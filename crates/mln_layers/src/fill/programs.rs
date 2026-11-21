//! The programs of the `fill` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO, MAX_UBO_COUNT_PER_SHADER,
};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_FILL_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const ID_FILL_EVALUATED_PROPS_UBO: usize = ID_FILL_TILE_PROPS_UBO + 1;
pub const FILL_LAYER_UBO_COUNT: usize = ID_FILL_EVALUATED_PROPS_UBO + 1;
pub const ID_FILL_IMAGE_TEXTURE: usize = 0;
pub const ID_FILL_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub const ID_FILL_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub const ID_FILL_OPACITY_VERTEX_ATTRIBUTE: usize = 2;
pub const ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE: usize = 3;
pub const ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 4;
pub const ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 5;
/// An outline instance's vertex, and where its segment of the
/// outline ends, which the instanced outline shaders read per instance.
pub const ID_FILL_OUTLINE_POS_ATTRIBUTE: usize = 6;
pub const ID_FILL_OUTLINE_END_ATTRIBUTE: usize = 7;

// The vertex buffers that `FillOutlinePulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub const ID_FILL_OUTLINE_VERTICES: usize = MAX_UBO_COUNT_PER_SHADER;
pub const ID_FILL_OUTLINE_ENDS: usize = MAX_UBO_COUNT_PER_SHADER + 1;
pub const ID_FILL_OUTLINE_PAINT: usize = MAX_UBO_COUNT_PER_SHADER + 2;

pub static FILL: Program = Program {
    name: "FillShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        Property::new("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        Property::new("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[],
    properties: &[
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        Property::new("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::new("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};
