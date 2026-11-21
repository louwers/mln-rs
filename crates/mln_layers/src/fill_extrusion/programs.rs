//! The programs of the `fill-extrusion` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::{Program, Property};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO, MAX_UBO_COUNT_PER_SHADER,
};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_FILL_EXTRUSION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_FILL_EXTRUSION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT; // SSBO
pub const ID_FILL_EXTRUSION_PROPS_UBO: usize = ID_FILL_EXTRUSION_TILE_PROPS_UBO + 1;
pub const FILL_EXTRUSION_LAYER_UBO_COUNT: usize = ID_FILL_EXTRUSION_PROPS_UBO + 1;

// The vertex buffers that `FillExtrusionPulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub const ID_FILL_EXTRUSION_WALL_VERTICES: usize = MAX_UBO_COUNT_PER_SHADER;
pub const ID_FILL_EXTRUSION_WALL_PAINT: usize = MAX_UBO_COUNT_PER_SHADER + 1;
pub const ID_FILL_EXTRUSION_IMAGE_TEXTURE: usize = 0;
pub const ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE: usize = 1;
/// The outline vertex a wall instance starts at.
pub const ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE: usize = 2;
/// The next outline vertex of a wall instance, which every
/// backend's shaders read as attributes.
pub const ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE: usize = 3;
pub const ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE: usize = 4;

// Data-driven properties.
pub const ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE: usize = 5;
pub const ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE: usize = 6;
pub const ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE: usize = 7;
pub const ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 8;
pub const ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 9;

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
