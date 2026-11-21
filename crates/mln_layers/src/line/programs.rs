//! The programs of the `line` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UByte4, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Property, Sources, Stages, Texture};

use super::ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// A layer binds one buffer of `LineDrawableUnionUBO`s and one of
// `LineTilePropsUnionUBO`s for all its programs, each of which reads its own
// block of them.
pub(crate) const DRAWABLE: Buffer =
    Buffer::storage_of::<LineDrawableUBO, LineDrawableUnionUBO>("drawableVector", Stages::Vertex);
const GRADIENT_DRAWABLE: Buffer =
    Buffer::storage_of::<LineGradientDrawableUBO, LineDrawableUnionUBO>("drawableVector", Stages::Vertex);
const PATTERN_DRAWABLE: Buffer =
    Buffer::storage_of::<LinePatternDrawableUBO, LineDrawableUnionUBO>("drawableVector", Stages::Vertex);
const SDF_DRAWABLE: Buffer =
    Buffer::storage_of::<LineSDFDrawableUBO, LineDrawableUnionUBO>("drawableVector", Stages::Vertex);
pub(crate) const PATTERN_TILE_PROPS: Buffer =
    Buffer::storage_of::<LinePatternTilePropsUBO, LineTilePropsUnionUBO>("tilePropsVector", Stages::Fragment);
const SDF_TILE_PROPS: Buffer =
    Buffer::storage_of::<LineSDFTilePropsUBO, LineTilePropsUnionUBO>("tilePropsVector", Stages::Fragment);
const _: () = assert!(
    DRAWABLE.id == GRADIENT_DRAWABLE.id
        && DRAWABLE.id == PATTERN_DRAWABLE.id
        && DRAWABLE.id == SDF_DRAWABLE.id
        && PATTERN_TILE_PROPS.id == SDF_TILE_PROPS.id
);
pub(crate) const PROPS: Buffer = Buffer::uniform::<LineEvaluatedPropsUBO>("props", 0);

// The ids of the programs' textures and vertex attributes.
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
    name: "line",
    buffers: &[DRAWABLE, PROPS],
    attributes: &[
        Attribute { id: ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_LINE_DATA_VERTEX_ATTRIBUTE, format: UByte4 },
        Attribute { id: ID_LINE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_LINE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OFFSET_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[],
    properties: LINE_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_SHADER)
        .msl(&msl::LINE_SHADER)
        .glsl_es(&glsl_es::LINE_SHADER)
        .glsl(&glsl::LINE_SHADER),
};

pub static LINE_GRADIENT: Program = Program {
    name: "line-gradient",
    buffers: &[GRADIENT_DRAWABLE, PROPS],
    attributes: &[
        Attribute { id: ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_LINE_DATA_VERTEX_ATTRIBUTE, format: UByte4 },
        Attribute { id: ID_LINE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OFFSET_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT_SHADER)
        .msl(&msl::LINE_GRADIENT_SHADER)
        .glsl_es(&glsl_es::LINE_GRADIENT_SHADER)
        .glsl(&glsl::LINE_GRADIENT_SHADER),
};

pub static LINE_PATTERN: Program = Program {
    name: "line-pattern",
    buffers: &[PATTERN_DRAWABLE, PATTERN_TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_LINE_DATA_VERTEX_ATTRIBUTE, format: UByte4 },
        Attribute { id: ID_LINE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OFFSET_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, format: UShort4 },
        Attribute { id: ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, format: UShort4 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        Property::vec4("pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_PATTERN_SHADER)
        .msl(&msl::LINE_PATTERN_SHADER)
        .glsl_es(&glsl_es::LINE_PATTERN_SHADER)
        .glsl(&glsl::LINE_PATTERN_SHADER),
};

pub static LINE_SDF: Program = Program {
    name: "line-sdf",
    buffers: &[SDF_DRAWABLE, SDF_TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_LINE_DATA_VERTEX_ATTRIBUTE, format: UByte4 },
        Attribute { id: ID_LINE_COLOR_VERTEX_ATTRIBUTE, format: Float4 },
        Attribute { id: ID_LINE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OFFSET_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, format: Float4 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
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
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_SDF_SHADER)
        .msl(&msl::LINE_SDF_SHADER)
        .glsl_es(&glsl_es::LINE_SDF_SHADER)
        .glsl(&glsl::LINE_SDF_SHADER),
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "line-gradient-sdf",
    buffers: &[SDF_DRAWABLE, SDF_TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_LINE_DATA_VERTEX_ATTRIBUTE, format: UByte4 },
        Attribute { id: ID_LINE_BLUR_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OPACITY_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_OFFSET_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, format: Float2 },
        Attribute { id: ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, format: Float4 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image"), Texture::new("image_dash")],
    properties: &[
        Property::float("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        Property::float("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        Property::float("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        Property::float("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        Property::float("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        Property::vec4("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT_SDF_SHADER)
        .msl(&msl::LINE_GRADIENT_SDF_SHADER)
        .glsl_es(&glsl_es::LINE_GRADIENT_SDF_SHADER)
        .glsl(&glsl::LINE_GRADIENT_SDF_SHADER),
};
