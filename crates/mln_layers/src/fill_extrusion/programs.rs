//! The programs of the `fill-extrusion` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Texture};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO, MAX_UBO_COUNT_PER_SHADER,
};

use super::ubo::{FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_FILL_EXTRUSION_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_FILL_EXTRUSION_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_FILL_EXTRUSION_PROPS_UBO: usize = ID_FILL_EXTRUSION_TILE_PROPS_UBO + 1;

// The vertex buffers that `FillExtrusionPulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub(crate) const ID_FILL_EXTRUSION_WALL_VERTICES: usize = MAX_UBO_COUNT_PER_SHADER;
pub(crate) const ID_FILL_EXTRUSION_WALL_PAINT: usize = MAX_UBO_COUNT_PER_SHADER + 1;
pub(crate) const ID_FILL_EXTRUSION_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE: usize = 1;
/// The outline vertex a wall instance starts at.
pub(crate) const ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE: usize = 2;
/// The next outline vertex of a wall instance, which every
/// backend's shaders read as attributes.
pub(crate) const ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE: usize = 3;
pub(crate) const ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE: usize = 4;

// Data-driven properties.
pub(crate) const ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE: usize = 6;
pub(crate) const ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE: usize = 7;
pub(crate) const ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 8;
pub(crate) const ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 9;

pub static FILL_EXTRUSION: Program = Program {
    name: "FillExtrusionShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_SHADER,
        msl: FILL_EXTRUSION_SHADER,
        glsl_es: FILL_EXTRUSION_SHADER,
        glsl: FILL_EXTRUSION_SHADER,
    ),
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2)],
    instance_attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, "a_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, "a_next_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, "a_next_decimals_ed", UShort2),
    ],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_INSTANCED_SHADER,
        msl: FILL_EXTRUSION_INSTANCED_SHADER,
        glsl_es: FILL_EXTRUSION_INSTANCED_SHADER,
        glsl: FILL_EXTRUSION_INSTANCED_SHADER,
    ),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, "a_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, "a_next_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, "a_next_decimals_ed", UShort2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_INSTANCED_SHADER,
        glsl_es: FILL_EXTRUSION_INSTANCED_SHADER,
        glsl: FILL_EXTRUSION_INSTANCED_SHADER,
    ),
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
        Buffer::vertices::<[i16; 4]>(ID_FILL_EXTRUSION_WALL_VERTICES),
        Buffer::vertices::<f32>(ID_FILL_EXTRUSION_WALL_PAINT),
    ],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::color("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_PULLED_SHADER,
        msl: FILL_EXTRUSION_PULLED_SHADER,
        glsl: FILL_EXTRUSION_PULLED_SHADER,
    ),
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, "a_pattern_from", UShort4),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, "a_pattern_to", UShort4),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_PATTERN_SHADER,
        msl: FILL_EXTRUSION_PATTERN_SHADER,
        glsl_es: FILL_EXTRUSION_PATTERN_SHADER,
        glsl: FILL_EXTRUSION_PATTERN_SHADER,
    ),
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2)],
    instance_attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, "a_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, "a_pattern_from", UShort4),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, "a_pattern_to", UShort4),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, "a_next_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, "a_next_decimals_ed", UShort2),
    ],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
        msl: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
        glsl_es: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
        glsl: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    ),
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
    buffers: &[
        Buffer::storage::<FillExtrusionDrawableUBO>(ID_FILL_EXTRUSION_DRAWABLE_UBO),
        Buffer::storage::<FillExtrusionTilePropsUBO>(ID_FILL_EXTRUSION_TILE_PROPS_UBO),
        Buffer::uniform::<FillExtrusionPropsUBO>(ID_FILL_EXTRUSION_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE, "a_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, "a_decimals_ed", UShort2),
        Attribute::new(ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, "a_base", Float2),
        Attribute::new(ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE, "a_height", Float2),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, "a_pattern_from", UShort4),
        Attribute::new(ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE, "a_pattern_to", UShort4),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, "a_next_outline_pos", Short2),
        Attribute::new(ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE, "a_next_decimals_ed", UShort2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::float("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        Property::float("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
        glsl_es: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
        glsl: FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    ),
};
