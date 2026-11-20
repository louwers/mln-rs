//! The programs of the `fill` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE,
};

use crate::shaders::property;

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
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
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
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
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
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
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
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
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
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
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
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
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
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};
