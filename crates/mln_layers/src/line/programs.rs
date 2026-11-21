//! The programs of the `line` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UByte4, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Texture};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
};

use super::ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_LINE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_LINE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO;
pub(crate) const ID_LINE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_LINE_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_LINE_DASH_TEXTURE: usize = 1;
pub(crate) const ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_LINE_DATA_VERTEX_ATTRIBUTE: usize = 1;

// Data-driven properties.
pub(crate) const ID_LINE_COLOR_VERTEX_ATTRIBUTE: usize = 2;
pub(crate) const ID_LINE_BLUR_VERTEX_ATTRIBUTE: usize = 3;
pub(crate) const ID_LINE_OPACITY_VERTEX_ATTRIBUTE: usize = 4;
pub(crate) const ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE: usize = 5;
pub(crate) const ID_LINE_OFFSET_VERTEX_ATTRIBUTE: usize = 6;
pub(crate) const ID_LINE_WIDTH_VERTEX_ATTRIBUTE: usize = 7;
pub(crate) const ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE: usize = 8;
pub(crate) const ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE: usize = 9;
pub(crate) const ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE: usize = 10;
pub(crate) const ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE: usize = 11;

const LINE_PROPERTIES: &[Property] = &[
    Property::color("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];

pub static LINE: Program = Program {
    name: "LineShader",
    buffers: &[
        Buffer::storage_of::<LineDrawableUBO, LineDrawableUnionUBO>(ID_LINE_DRAWABLE_UBO),
        Buffer::uniform::<LineEvaluatedPropsUBO>(ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, "a_pos_normal", Short2),
        Attribute::new(ID_LINE_DATA_VERTEX_ATTRIBUTE, "a_data", UByte4),
        Attribute::new(ID_LINE_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_LINE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_LINE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, "a_gapwidth", Float2),
        Attribute::new(ID_LINE_OFFSET_VERTEX_ATTRIBUTE, "a_offset", Float2),
        Attribute::new(ID_LINE_WIDTH_VERTEX_ATTRIBUTE, "a_width", Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: LINE_PROPERTIES,
    sources: sources!(wgsl: LINE_SHADER, msl: LINE_SHADER, glsl_es: LINE_SHADER, glsl: LINE_SHADER,),
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    buffers: &[
        Buffer::storage_of::<LineGradientDrawableUBO, LineDrawableUnionUBO>(ID_LINE_DRAWABLE_UBO),
        Buffer::uniform::<LineEvaluatedPropsUBO>(ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, "a_pos_normal", Short2),
        Attribute::new(ID_LINE_DATA_VERTEX_ATTRIBUTE, "a_data", UByte4),
        Attribute::new(ID_LINE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_LINE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, "a_gapwidth", Float2),
        Attribute::new(ID_LINE_OFFSET_VERTEX_ATTRIBUTE, "a_offset", Float2),
        Attribute::new(ID_LINE_WIDTH_VERTEX_ATTRIBUTE, "a_width", Float2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: LINE_GRADIENT_SHADER,
        msl: LINE_GRADIENT_SHADER,
        glsl_es: LINE_GRADIENT_SHADER,
        glsl: LINE_GRADIENT_SHADER,
    ),
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    buffers: &[
        Buffer::storage_of::<LinePatternDrawableUBO, LineDrawableUnionUBO>(ID_LINE_DRAWABLE_UBO),
        Buffer::storage_of::<LinePatternTilePropsUBO, LineTilePropsUnionUBO>(ID_LINE_TILE_PROPS_UBO),
        Buffer::uniform::<LineEvaluatedPropsUBO>(ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, "a_pos_normal", Short2),
        Attribute::new(ID_LINE_DATA_VERTEX_ATTRIBUTE, "a_data", UByte4),
        Attribute::new(ID_LINE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_LINE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, "a_gapwidth", Float2),
        Attribute::new(ID_LINE_OFFSET_VERTEX_ATTRIBUTE, "a_offset", Float2),
        Attribute::new(ID_LINE_WIDTH_VERTEX_ATTRIBUTE, "a_width", Float2),
        Attribute::new(ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, "a_pattern_from", UShort4),
        Attribute::new(ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, "a_pattern_to", UShort4),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: LINE_PATTERN_SHADER,
        msl: LINE_PATTERN_SHADER,
        glsl_es: LINE_PATTERN_SHADER,
        glsl: LINE_PATTERN_SHADER,
    ),
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    buffers: &[
        Buffer::storage_of::<LineSDFDrawableUBO, LineDrawableUnionUBO>(ID_LINE_DRAWABLE_UBO),
        Buffer::storage_of::<LineSDFTilePropsUBO, LineTilePropsUnionUBO>(ID_LINE_TILE_PROPS_UBO),
        Buffer::uniform::<LineEvaluatedPropsUBO>(ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, "a_pos_normal", Short2),
        Attribute::new(ID_LINE_DATA_VERTEX_ATTRIBUTE, "a_data", UByte4),
        Attribute::new(ID_LINE_COLOR_VERTEX_ATTRIBUTE, "a_color", Float4),
        Attribute::new(ID_LINE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_LINE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, "a_gapwidth", Float2),
        Attribute::new(ID_LINE_OFFSET_VERTEX_ATTRIBUTE, "a_offset", Float2),
        Attribute::new(ID_LINE_WIDTH_VERTEX_ATTRIBUTE, "a_width", Float2),
        Attribute::new(ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, "a_floorwidth", Float2),
        Attribute::new(ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, "a_dasharray", Float4),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[
        Property::color("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        Property::vec4("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: LINE_SDF_SHADER,
        msl: LINE_SDF_SHADER,
        glsl_es: LINE_SDF_SHADER,
        glsl: LINE_SDF_SHADER,
    ),
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    buffers: &[
        Buffer::storage_of::<LineSDFDrawableUBO, LineDrawableUnionUBO>(ID_LINE_DRAWABLE_UBO),
        Buffer::storage_of::<LineSDFTilePropsUBO, LineTilePropsUnionUBO>(ID_LINE_TILE_PROPS_UBO),
        Buffer::uniform::<LineEvaluatedPropsUBO>(ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, "a_pos_normal", Short2),
        Attribute::new(ID_LINE_DATA_VERTEX_ATTRIBUTE, "a_data", UByte4),
        Attribute::new(ID_LINE_BLUR_VERTEX_ATTRIBUTE, "a_blur", Float2),
        Attribute::new(ID_LINE_OPACITY_VERTEX_ATTRIBUTE, "a_opacity", Float2),
        Attribute::new(ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, "a_gapwidth", Float2),
        Attribute::new(ID_LINE_OFFSET_VERTEX_ATTRIBUTE, "a_offset", Float2),
        Attribute::new(ID_LINE_WIDTH_VERTEX_ATTRIBUTE, "a_width", Float2),
        Attribute::new(ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, "a_floorwidth", Float2),
        Attribute::new(ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, "a_dasharray", Float4),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image"), Texture::new("u_image_dash")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        Property::vec4("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
    sources: sources!(
        wgsl: LINE_GRADIENT_SDF_SHADER,
        msl: LINE_GRADIENT_SDF_SHADER,
        glsl_es: LINE_GRADIENT_SDF_SHADER,
        glsl: LINE_GRADIENT_SDF_SHADER,
    ),
};
