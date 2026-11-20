//! The programs of the `fill-extrusion` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE,
};

pub static FILL_EXTRUSION: Program = Program {
    name: "FillExtrusionShader",
    attributes: &[
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
    attributes: &[(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
    attributes: &[
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
    attributes: &[
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
    attributes: &[(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
    ],
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
    attributes: &[
        (ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, UShort2),
        (ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_EXTRUSION_IMAGE_TEXTURE],
    properties: &[
        Property::new("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::new("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::new("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::new("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};
