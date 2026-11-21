//! The programs of the `fill` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::bindings::{FIRST_UNIFORM_ID, FIRST_VERTEX_BUFFER_ID, VERTEX_STORAGE_ID};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Sources, Texture};

use super::ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_FILL_DRAWABLE_UBO: usize = VERTEX_STORAGE_ID;
pub(crate) const ID_FILL_TILE_PROPS_UBO: usize = FIRST_UNIFORM_ID;
pub(crate) const ID_FILL_EVALUATED_PROPS_UBO: usize = ID_FILL_TILE_PROPS_UBO + 1;
pub(crate) const ID_FILL_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_FILL_POS_VERTEX_ATTRIBUTE: usize = 0;

// Data-driven properties.
pub(crate) const ID_FILL_COLOR_VERTEX_ATTRIBUTE: usize = 1;
pub(crate) const ID_FILL_OPACITY_VERTEX_ATTRIBUTE: usize = 2;
pub(crate) const ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE: usize = 3;
pub(crate) const ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 4;
pub(crate) const ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 5;
/// An outline instance's vertex, and where its segment of the
/// outline ends, which the instanced outline shaders read per instance.
pub(crate) const ID_FILL_OUTLINE_POS_ATTRIBUTE: usize = 6;
pub(crate) const ID_FILL_OUTLINE_END_ATTRIBUTE: usize = 7;

// The vertex buffers that `FillOutlinePulledShader` reads as storage
// buffers, whose ids follow the uniform buffers'.
pub(crate) const ID_FILL_OUTLINE_VERTICES: usize = FIRST_VERTEX_BUFFER_ID;
pub(crate) const ID_FILL_OUTLINE_ENDS: usize = FIRST_VERTEX_BUFFER_ID + 1;
pub(crate) const ID_FILL_OUTLINE_PAINT: usize = FIRST_VERTEX_BUFFER_ID + 2;

pub static FILL: Program = Program {
    name: "fill",
    buffers: &[
        Buffer::storage_of::<FillDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::color("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_SHADER)
        .msl(&msl::FILL_SHADER)
        .glsl_es(&glsl_es::FILL_SHADER)
        .glsl(&glsl::FILL_SHADER),
};

pub static FILL_OUTLINE: Program = Program {
    name: "fill-outline",
    buffers: &[
        Buffer::storage_of::<FillOutlineDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::color("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_SHADER)
        .glsl(&glsl::FILL_OUTLINE_SHADER),
};

pub static FILL_PATTERN: Program = Program {
    name: "fill-pattern",
    buffers: &[
        Buffer::storage_of::<FillPatternDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::storage_of::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>(ID_FILL_TILE_PROPS_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::color("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_PATTERN_SHADER)
        .msl(&msl::FILL_PATTERN_SHADER)
        .glsl_es(&glsl_es::FILL_PATTERN_SHADER)
        .glsl(&glsl::FILL_PATTERN_SHADER),
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "fill-outline-pattern",
    buffers: &[
        Buffer::storage_of::<FillPatternDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::storage_of::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>(ID_FILL_TILE_PROPS_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::color("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PATTERN_SHADER),
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "fill-outline-instanced",
    buffers: &[
        Buffer::storage_of::<FillOutlineDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[
        Attribute { id: ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_OUTLINE_END_ATTRIBUTE, format: Short2 },
    ],
    textures: &[],
    properties: &[
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::color("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_INSTANCED_SHADER)
        .msl(&msl::FILL_OUTLINE_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_INSTANCED_SHADER)
        .glsl(&glsl::FILL_OUTLINE_INSTANCED_SHADER),
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "fill-outline-pulled",
    buffers: &[
        Buffer::storage_of::<FillOutlineDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
        Buffer::vertices::<[i16; 2]>(ID_FILL_OUTLINE_VERTICES),
        Buffer::vertices::<[i16; 2]>(ID_FILL_OUTLINE_ENDS),
        Buffer::vertices::<f32>(ID_FILL_OUTLINE_PAINT),
    ],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::color("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PULLED_SHADER)
        .msl(&msl::FILL_OUTLINE_PULLED_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PULLED_SHADER),
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "fill-outline-pattern-instanced",
    buffers: &[
        Buffer::storage_of::<FillPatternDrawableUBO, FillDrawableUnionUBO>(ID_FILL_DRAWABLE_UBO),
        Buffer::storage_of::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>(ID_FILL_TILE_PROPS_UBO),
        Buffer::uniform::<FillEvaluatedPropsUBO>(ID_FILL_EVALUATED_PROPS_UBO),
    ],
    attributes: &[Attribute { id: ID_FILL_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[
        Attribute { id: ID_FILL_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_FILL_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_FILL_OUTLINE_POS_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_FILL_OUTLINE_END_ATTRIBUTE, format: Short2 },
    ],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::color("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .msl(&msl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
};
