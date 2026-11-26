//! The fills' programs.

use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UShort4};
use mln_gfx::shaders::program::{
    Attribute, Program, ProgramBuffer, Projection, Property, Stages, Variants, VertexWord,
};
use mln_gfx::shaders::source::Sources;

use super::ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use super::{glsl, glsl_es, msl, wgsl};
use crate::shaders::programs::{IMAGE, POS};

/// The fills' buffers: the draws of a layer's fill programs write one buffer
/// of draws and one of tiles. The tiles' buffer takes the uniform block's
/// slot 0. The patterns' vertex stages read the uniform block too.
const FILL_PROPS: ProgramBuffer = ProgramBuffer::uniform::<FillEvaluatedPropsUBO>("props", 1, Stages::Fragment);

const FILL_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<FillDrawableUBO, FillDrawableUnionUBO>("drawableVector", Stages::Vertex),
    FILL_PROPS,
];

const FILL_OUTLINE_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<FillOutlineDrawableUBO, FillDrawableUnionUBO>("drawableVector", Stages::Vertex),
    FILL_PROPS,
];

const FILL_PATTERN_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<FillPatternDrawableUBO, FillDrawableUnionUBO>("drawableVector", Stages::Vertex),
    ProgramBuffer::union_storage::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>("tilePropsVector", Stages::Both),
    ProgramBuffer { stages: Stages::Both, ..FILL_PROPS },
];

const FILL_COLOR: Attribute = Attribute::new("color", Float4);

const FILL_OPACITY: Attribute = Attribute::new("opacity", Float2);

const FILL_OUTLINE_COLOR: Attribute = Attribute::new("outline_color", Float4);

const FILL_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);

const FILL_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);

/// An outline instance's vertex, and where its segment of the outline ends.
const FILL_OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);

const FILL_OUTLINE_END: Attribute = Attribute::new("outline_end", Short2);

const FILL_PROPERTIES: &[Property] = &[Property::new("color"), Property::new("opacity")];

const FILL_OUTLINE_PROPERTIES: &[Property] = &[Property::new("opacity"), Property::new("outline_color")];

const FILL_PATTERN_PROPERTIES: &[Property] =
    &[Property::new("color"), Property::new("opacity"), Property::new("pattern_from"), Property::new("pattern_to")];

pub static FILL: Program = Program {
    name: "FillShader",
    buffers: FILL_BUFFERS,
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_PROPERTIES,
    sources: Sources::NONE.wgsl(&wgsl::FILL).msl(&msl::FILL).glsl(&glsl::FILL).glsl_es(&glsl_es::FILL),
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    buffers: FILL_OUTLINE_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_OUTLINE_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
    // No MSL: Metal draws no lines wider than 1 px, so it draws every
    // outline as quads.
    sources: Sources::NONE.wgsl(&wgsl::FILL_OUTLINE).glsl(&glsl::FILL_OUTLINE).glsl_es(&glsl_es::FILL_OUTLINE),
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    buffers: FILL_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_PATTERN)
        .msl(&msl::FILL_PATTERN)
        .glsl(&glsl::FILL_PATTERN)
        .glsl_es(&glsl_es::FILL_PATTERN),
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    buffers: FILL_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
    // No MSL, as for `FILL_OUTLINE`.
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN)
        .glsl(&glsl::FILL_OUTLINE_PATTERN)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN),
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    buffers: FILL_OUTLINE_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[FILL_OUTLINE_COLOR, FILL_OPACITY, FILL_OUTLINE_POS, FILL_OUTLINE_END],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_INSTANCED)
        .msl(&msl::FILL_OUTLINE_INSTANCED)
        .glsl(&glsl::FILL_OUTLINE_INSTANCED)
        .glsl_es(&glsl_es::FILL_OUTLINE_INSTANCED),
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    buffers: &[
        FILL_OUTLINE_BUFFERS[0],
        FILL_OUTLINE_BUFFERS[1],
        ProgramBuffer::vertices("outlineVertices", 0, VertexWord::Uint),
        ProgramBuffer::vertices("outlineEnds", 1, VertexWord::Uint),
        ProgramBuffer::vertices("outlinePaint", 2, VertexWord::Float).only_in(Variants::PerVertex),
    ],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
    // No GL ES, whose vertex stages read no storage buffers.
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PULLED)
        .msl(&msl::FILL_OUTLINE_PULLED)
        .glsl(&glsl::FILL_OUTLINE_PULLED),
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    buffers: FILL_PATTERN_BUFFERS,
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[
        FILL_COLOR,
        FILL_PATTERN_FROM,
        FILL_PATTERN_TO,
        FILL_OPACITY,
        FILL_OUTLINE_POS,
        FILL_OUTLINE_END,
    ],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
    sources: Sources::NONE
        .wgsl(&wgsl::FILL_OUTLINE_PATTERN_INSTANCED)
        .msl(&msl::FILL_OUTLINE_PATTERN_INSTANCED)
        .glsl(&glsl::FILL_OUTLINE_PATTERN_INSTANCED)
        .glsl_es(&glsl_es::FILL_OUTLINE_PATTERN_INSTANCED),
};
