//! The lines' programs.

use mln_engine::shaders::programs::IMAGE;
use mln_gfx::gfx_types::AttributeDataType::{Float2, Float4, Short2, UByte4, UShort4};
use mln_gfx::shaders::block::UniformBlock;
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

/// A vertex's position, doubled, with its round and up flags in the low
/// bits.
/// A line program's buffer of draws, of `T`s: the draws of a layer's line
/// programs write one buffer of draws and one of tiles.
const fn line_drawable<T: UniformBlock>() -> ProgramBuffer {
    ProgramBuffer::union_storage::<T, LineDrawableUnionUBO>("drawableVector", Stages::Vertex)
}

/// A line program's buffer of tiles, of `T`s.
const fn line_tile_props<T: UniformBlock>() -> ProgramBuffer {
    ProgramBuffer::union_storage::<T, LineTilePropsUnionUBO>("tilePropsVector", Stages::Fragment)
}

const LINE_PROPS: ProgramBuffer = ProgramBuffer::uniform::<LineEvaluatedPropsUBO>("props", 0, Stages::Both);

const LINE_POS_NORMAL: Attribute = Attribute::new("pos_normal", Short2);

/// A vertex's extrusion, direction and distance along the line.
const LINE_DATA: Attribute = Attribute::new("data", UByte4);

const LINE_COLOR: Attribute = Attribute::new("color", Float4);

const LINE_BLUR: Attribute = Attribute::new("blur", Float2);

const LINE_OPACITY: Attribute = Attribute::new("opacity", Float2);

const LINE_GAP_WIDTH: Attribute = Attribute::new("gapwidth", Float2);

const LINE_OFFSET: Attribute = Attribute::new("offset", Float2);

const LINE_WIDTH: Attribute = Attribute::new("width", Float2);

const LINE_FLOOR_WIDTH: Attribute = Attribute::new("floorwidth", Float2);

const LINE_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);

const LINE_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);

/// Where a feature's dash pattern is in its tile's atlas.
const LINE_DASHARRAY: Attribute = Attribute::new("dasharray", Float4);

pub static LINE: Program = Program {
    name: "LineShader",
    buffers: &[line_drawable::<LineDrawableUBO>(), LINE_PROPS],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_COLOR,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        Property::new("color"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
    ],
    sources: Sources::NONE.wgsl(&wgsl::LINE).msl(&msl::LINE).glsl(&glsl::LINE).glsl_es(&glsl_es::LINE),
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    buffers: &[line_drawable::<LineGradientDrawableUBO>(), LINE_PROPS],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[LINE_POS_NORMAL, LINE_DATA, LINE_BLUR, LINE_OPACITY, LINE_GAP_WIDTH, LINE_OFFSET, LINE_WIDTH],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT)
        .msl(&msl::LINE_GRADIENT)
        .glsl(&glsl::LINE_GRADIENT)
        .glsl_es(&glsl_es::LINE_GRADIENT),
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    buffers: &[line_drawable::<LinePatternDrawableUBO>(), line_tile_props::<LinePatternTilePropsUBO>(), LINE_PROPS],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_PATTERN_FROM,
        LINE_PATTERN_TO,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("pattern_from"),
        Property::new("pattern_to"),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_PATTERN)
        .msl(&msl::LINE_PATTERN)
        .glsl(&glsl::LINE_PATTERN)
        .glsl_es(&glsl_es::LINE_PATTERN),
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    buffers: &[line_drawable::<LineSDFDrawableUBO>(), line_tile_props::<LineSDFTilePropsUBO>(), LINE_PROPS],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_COLOR,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_FLOOR_WIDTH,
        LINE_DASHARRAY,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[
        Property::new("color"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("floorwidth"),
        Property::new("dasharray"),
    ],
    sources: Sources::NONE.wgsl(&wgsl::LINE_SDF).msl(&msl::LINE_SDF).glsl(&glsl::LINE_SDF).glsl_es(&glsl_es::LINE_SDF),
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    buffers: &[line_drawable::<LineSDFDrawableUBO>(), line_tile_props::<LineSDFTilePropsUBO>(), LINE_PROPS],
    paint_params: Some(Stages::Vertex),
    projection: Projection::PerDraw,
    attributes: &[
        LINE_POS_NORMAL,
        LINE_DATA,
        LINE_BLUR,
        LINE_OPACITY,
        LINE_GAP_WIDTH,
        LINE_OFFSET,
        LINE_WIDTH,
        LINE_FLOOR_WIDTH,
        LINE_DASHARRAY,
    ],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("image_dash", Stages::Fragment)],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("floorwidth"),
        Property::new("dasharray"),
    ],
    sources: Sources::NONE
        .wgsl(&wgsl::LINE_GRADIENT_SDF)
        .msl(&msl::LINE_GRADIENT_SDF)
        .glsl(&glsl::LINE_GRADIENT_SDF)
        .glsl_es(&glsl_es::LINE_GRADIENT_SDF),
};
