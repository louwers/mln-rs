//! The programs of the `fill` layer type: the buffers, attributes, textures
//! and paint properties of its shaders, and their sources.
//!
//! A fill draws the bucket's triangles, and its outline the bucket's lines,
//! or a quad from each vertex to its outline end: as instances of one quad,
//! an instance per vertex, or, where the backend binds vertex buffers as
//! storage buffers, as quads without attributes, whose shader reads the
//! bucket's vertices, outline ends and paint values by index.

use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Property, Sources, Stages, Texture};

use super::ubo::{
    FillDrawableUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO, FillPatternTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

const DRAWABLE: Buffer = Buffer::storage::<FillDrawableUBO>("drawableVector", Stages::Vertex);
const OUTLINE_DRAWABLE: Buffer = Buffer::storage::<FillOutlineDrawableUBO>("drawableVector", Stages::Vertex);
const PATTERN_DRAWABLE: Buffer = Buffer::storage::<FillPatternDrawableUBO>("drawableVector", Stages::Vertex);
const PATTERN_TILE_PROPS: Buffer = Buffer::storage::<FillPatternTilePropsUBO>("tilePropsVector", Stages::Both);
const PROPS: Buffer = Buffer::uniform::<FillEvaluatedPropsUBO>("props", 1);
/// The bucket's vertices, their outline ends and their paint values, which
/// the pulled outlines read.
const OUTLINE_VERTICES: Buffer = Buffer::vertices("outlineVertices", 0, Short2);
const OUTLINE_ENDS: Buffer = Buffer::vertices("outlineEnds", 1, Short2);
const OUTLINE_PAINT: Buffer = Buffer::vertices("outlinePaint", 2, Float);

/// A vertex's position.
const POSITION: Attribute = Attribute::new("position", Short2);
/// A corner of the instanced outlines' quad.
const CORNER: Attribute = Attribute::new("corner", Short2);
/// The vertex an instanced outline's segment starts at, and its end.
const OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);
const OUTLINE_END: Attribute = Attribute::new("outline_end", Short2);
const COLOR: Attribute = Attribute::new("color", Float4);
const OPACITY: Attribute = Attribute::new("opacity", Float2);
const OUTLINE_COLOR: Attribute = Attribute::new("outline_color", Float4);
const PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);

const OUTLINE: &[Property] = &[Property::float("opacity"), Property::color("outline_color")];
const PATTERNED: &[Property] = &[
    Property::color("color"),
    Property::float("opacity"),
    Property::vec4("pattern_from"),
    Property::vec4("pattern_to"),
];

pub static FILL: Program = Program {
    name: "fill",
    buffers: &[DRAWABLE, PROPS],
    attributes: &[POSITION, COLOR, OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::color("color"), Property::float("opacity")],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_SHADER)
        .msl(&msl::FILL_SHADER)
        .glsl_es(&glsl_es::FILL_SHADER)
        .glsl(&glsl::FILL_SHADER),
};

pub static FILL_OUTLINE: Program = Program {
    name: "fill-outline",
    buffers: &[OUTLINE_DRAWABLE, PROPS],
    attributes: &[POSITION, OUTLINE_COLOR, OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: OUTLINE,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_SHADER)
        .glsl(&glsl::FILL_OUTLINE_SHADER),
};

pub static FILL_PATTERN: Program = Program {
    name: "fill-pattern",
    buffers: &[PATTERN_DRAWABLE, PATTERN_TILE_PROPS, PROPS],
    attributes: &[POSITION, COLOR, PATTERN_FROM, PATTERN_TO, OPACITY],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_PATTERN_SHADER)
        .msl(&msl::FILL_PATTERN_SHADER)
        .glsl_es(&glsl_es::FILL_PATTERN_SHADER)
        .glsl(&glsl::FILL_PATTERN_SHADER),
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "fill-outline-pattern",
    buffers: &[PATTERN_DRAWABLE, PATTERN_TILE_PROPS, PROPS],
    attributes: &[POSITION, COLOR, PATTERN_FROM, PATTERN_TO, OPACITY],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PATTERN_SHADER),
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "fill-outline-instanced",
    buffers: &[OUTLINE_DRAWABLE, PROPS],
    attributes: &[CORNER],
    instance_attributes: &[OUTLINE_COLOR, OPACITY, OUTLINE_POS, OUTLINE_END],
    textures: &[],
    properties: OUTLINE,
    projection: Projection::PerDraw,
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
    buffers: &[OUTLINE_DRAWABLE, PROPS, OUTLINE_VERTICES, OUTLINE_ENDS, OUTLINE_PAINT],
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: OUTLINE,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PULLED_SHADER)
        .msl(&msl::FILL_OUTLINE_PULLED_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PULLED_SHADER),
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "fill-outline-pattern-instanced",
    buffers: &[PATTERN_DRAWABLE, PATTERN_TILE_PROPS, PROPS],
    attributes: &[CORNER],
    instance_attributes: &[COLOR, PATTERN_FROM, PATTERN_TO, OPACITY, OUTLINE_POS, OUTLINE_END],
    textures: &[Texture::new("image")],
    properties: PATTERNED,
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .msl(&msl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN_INSTANCED_SHADER)
        .glsl(&glsl::FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
};
