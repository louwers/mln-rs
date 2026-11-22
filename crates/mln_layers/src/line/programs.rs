//! The programs of the `line` layer type: the buffers, attributes, textures
//! and paint properties of its shaders, and their sources.
//!
//! A line draws in its color, along its layer's gradient, with its layer's
//! pattern, or dashed: with the dash array's patterns as signed distance
//! fields, in its color or along the gradient.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UByte4, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages, Texture};

use super::ubo::{
    LineDrawableUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO, LinePatternTilePropsUBO,
    LineSDFDrawableUBO, LineSDFTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

const PROPS: Buffer = Buffer::uniform::<LineEvaluatedPropsUBO>("props", 0);
const SDF_DRAWABLE: Buffer = Buffer::storage::<LineSDFDrawableUBO>("drawableVector", Stages::Vertex);
const SDF_TILE_PROPS: Buffer = Buffer::storage::<LineSDFTilePropsUBO>("tilePropsVector", Stages::Fragment);

/// A vertex's position, doubled, with its round and up flags in the low
/// bits.
const POS_NORMAL: Attribute = Attribute::new("pos_normal", Short2);
/// A vertex's extrusion, direction and distance along the line.
const DATA: Attribute = Attribute::new("data", UByte4);
const COLOR: Attribute = Attribute::new("color", Float4);
const BLUR: Attribute = Attribute::new("blur", Float2);
const OPACITY: Attribute = Attribute::new("opacity", Float2);
const GAP_WIDTH: Attribute = Attribute::new("gapwidth", Float2);
const OFFSET: Attribute = Attribute::new("offset", Float2);
const WIDTH: Attribute = Attribute::new("width", Float2);
const FLOOR_WIDTH: Attribute = Attribute::new("floorwidth", Float2);
const PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);
/// Where a feature's dash pattern is in its tile's atlas, where the dash
/// array differs by feature.
const DASHARRAY: Attribute = Attribute::new("dasharray", Float4);

const COLOR_PROPERTY: Property = Property::color("color");
const BLUR_PROPERTY: Property = Property::float("blur");
const OPACITY_PROPERTY: Property = Property::float("opacity");
const GAP_WIDTH_PROPERTY: Property = Property::float("gapwidth");
const OFFSET_PROPERTY: Property = Property::float("offset");
const WIDTH_PROPERTY: Property = Property::float("width");
const FLOOR_WIDTH_PROPERTY: Property = Property::float("floorwidth");
const DASHARRAY_PROPERTY: Property = Property::vec4("dasharray");

pub static LINE: Program = Program {
    name: "line",
    buffers: &[Buffer::storage::<LineDrawableUBO>("drawableVector", Stages::Vertex), PROPS],
    attributes: &[POS_NORMAL, DATA, COLOR, BLUR, OPACITY, GAP_WIDTH, OFFSET, WIDTH],
    instance_attributes: &[],
    textures: &[],
    properties: &[COLOR_PROPERTY, BLUR_PROPERTY, OPACITY_PROPERTY, GAP_WIDTH_PROPERTY, OFFSET_PROPERTY, WIDTH_PROPERTY],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_SHADER)
        .msl(&msl::LINE_SHADER)
        .glsl_es(&glsl_es::LINE_SHADER)
        .glsl(&glsl::LINE_SHADER),
};

pub static LINE_GRADIENT: Program = Program {
    name: "line-gradient",
    buffers: &[Buffer::storage::<LineGradientDrawableUBO>("drawableVector", Stages::Vertex), PROPS],
    attributes: &[POS_NORMAL, DATA, BLUR, OPACITY, GAP_WIDTH, OFFSET, WIDTH],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[BLUR_PROPERTY, OPACITY_PROPERTY, GAP_WIDTH_PROPERTY, OFFSET_PROPERTY, WIDTH_PROPERTY],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT_SHADER)
        .msl(&msl::LINE_GRADIENT_SHADER)
        .glsl_es(&glsl_es::LINE_GRADIENT_SHADER)
        .glsl(&glsl::LINE_GRADIENT_SHADER),
};

pub static LINE_PATTERN: Program = Program {
    name: "line-pattern",
    buffers: &[
        Buffer::storage::<LinePatternDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::storage::<LinePatternTilePropsUBO>("tilePropsVector", Stages::Fragment),
        PROPS,
    ],
    attributes: &[POS_NORMAL, DATA, BLUR, OPACITY, GAP_WIDTH, OFFSET, WIDTH, PATTERN_FROM, PATTERN_TO],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        BLUR_PROPERTY,
        OPACITY_PROPERTY,
        GAP_WIDTH_PROPERTY,
        OFFSET_PROPERTY,
        WIDTH_PROPERTY,
        Property::vec4("pattern_from"),
        Property::vec4("pattern_to"),
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_PATTERN_SHADER)
        .msl(&msl::LINE_PATTERN_SHADER)
        .glsl_es(&glsl_es::LINE_PATTERN_SHADER)
        .glsl(&glsl::LINE_PATTERN_SHADER),
};

pub static LINE_SDF: Program = Program {
    name: "line-sdf",
    buffers: &[SDF_DRAWABLE, SDF_TILE_PROPS, PROPS],
    attributes: &[POS_NORMAL, DATA, COLOR, BLUR, OPACITY, GAP_WIDTH, OFFSET, WIDTH, FLOOR_WIDTH, DASHARRAY],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[
        COLOR_PROPERTY,
        BLUR_PROPERTY,
        OPACITY_PROPERTY,
        GAP_WIDTH_PROPERTY,
        OFFSET_PROPERTY,
        WIDTH_PROPERTY,
        FLOOR_WIDTH_PROPERTY,
        DASHARRAY_PROPERTY,
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_SDF_SHADER)
        .msl(&msl::LINE_SDF_SHADER)
        .glsl_es(&glsl_es::LINE_SDF_SHADER)
        .glsl(&glsl::LINE_SDF_SHADER),
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "line-gradient-sdf",
    buffers: &[SDF_DRAWABLE, SDF_TILE_PROPS, PROPS],
    attributes: &[POS_NORMAL, DATA, BLUR, OPACITY, GAP_WIDTH, OFFSET, WIDTH, FLOOR_WIDTH, DASHARRAY],
    instance_attributes: &[],
    textures: &[Texture::new("image"), Texture::new("image_dash")],
    properties: &[
        BLUR_PROPERTY,
        OPACITY_PROPERTY,
        GAP_WIDTH_PROPERTY,
        OFFSET_PROPERTY,
        WIDTH_PROPERTY,
        FLOOR_WIDTH_PROPERTY,
        DASHARRAY_PROPERTY,
    ],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT_SDF_SHADER)
        .msl(&msl::LINE_GRADIENT_SDF_SHADER)
        .glsl_es(&glsl_es::LINE_GRADIENT_SDF_SHADER)
        .glsl(&glsl::LINE_GRADIENT_SDF_SHADER),
};
