//! The built-in programs.

use crate::gfx_types::AttributeDataType::{Float, Float2, Float4, Short2, Short4, UByte4, UShort2, UShort4};
use crate::shaders::background_layer_ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use crate::shaders::block::UniformBlock;
use crate::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use crate::shaders::color_relief_layer_ubo::{
    ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO,
};
use crate::shaders::fill_extrusion_layer_ubo::{
    FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO,
};
use crate::shaders::fill_layer_ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use crate::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use crate::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use crate::shaders::hillshade_layer_ubo::{HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO};
use crate::shaders::hillshade_prepare_layer_ubo::{HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use crate::shaders::layer_composite_ubo::LayerCompositeUBO;
use crate::shaders::line_layer_ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use crate::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages};
use crate::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use crate::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use crate::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use crate::shaders::terrain_ubo::{TerrainDrawableUBO, TerrainSurfaceUBO};

const POS: Attribute = Attribute::new("pos", Short2);
const IMAGE: ProgramTexture = ProgramTexture::new("image");
/// Over terrain, the height of the ground under a feature's anchor, in
/// meters with the exaggeration, which the terrain variants of the programs
/// that stand on it read.
const GROUND_HEIGHT: Attribute = Attribute::new("ground_height", Float);
/// The ground's depth: each pixel's eye distance to the nearest ground drawn
/// there, the bits of an `f32`, least significant byte first, in an RGBA8
/// texel; 0 where none is. The terrain variants of the programs that hide
/// behind the ground read it, in the slot their own textures leave free.
const GROUND_DEPTH: ProgramTexture = ProgramTexture::unfilterable("ground_depth");

/// The background's buffers: a background and a background pattern draw
/// write one buffer of draws.
const BACKGROUND_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>("drawableVector", Stages::Vertex),
    ProgramBuffer::uniform::<BackgroundPropsUBO>("props", 0),
];
const BACKGROUND_PATTERN_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::union_storage::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(
        "drawableVector",
        Stages::Vertex,
    ),
    ProgramBuffer::uniform::<BackgroundPatternPropsUBO>("props", 0),
];

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    buffers: BACKGROUND_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    buffers: BACKGROUND_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    buffers: &[
        ProgramBuffer::storage::<CircleDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<CircleEvaluatedPropsUBO>("props", 0),
    ],
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        Attribute::new("color", Float4),
        Attribute::new("radius", Float2),
        Attribute::new("blur", Float2),
        Attribute::new("opacity", Float2),
        Attribute::new("stroke_color", Float4),
        Attribute::new("stroke_width", Float2),
        Attribute::new("stroke_opacity", Float2),
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[
        Property::new("color"),
        Property::new("radius"),
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("stroke_color"),
        Property::new("stroke_width"),
        Property::new("stroke_opacity"),
    ],
};

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    buffers: &[],
    projection: Projection::Own,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

/// The fills' buffers: the draws of a layer's fill programs write one buffer
/// of draws and one of tiles. The tiles' buffer takes the uniform block's
/// slot 0.
const FILL_PROPS: ProgramBuffer = ProgramBuffer::uniform::<FillEvaluatedPropsUBO>("props", 1);
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
    FILL_PROPS,
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
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_PROPERTIES,
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    buffers: FILL_OUTLINE_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_OUTLINE_COLOR, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    buffers: FILL_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    buffers: FILL_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS, FILL_COLOR, FILL_PATTERN_FROM, FILL_PATTERN_TO, FILL_OPACITY],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_PATTERN_PROPERTIES,
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    buffers: FILL_OUTLINE_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[FILL_OUTLINE_COLOR, FILL_OPACITY, FILL_OUTLINE_POS, FILL_OUTLINE_END],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    buffers: &[
        FILL_OUTLINE_BUFFERS[0],
        FILL_OUTLINE_BUFFERS[1],
        ProgramBuffer::vertices("outlineVertices", 0),
        ProgramBuffer::vertices("outlineEnds", 1),
        ProgramBuffer::vertices("outlinePaint", 2),
    ],
    projection: Projection::PerDraw,
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_OUTLINE_PROPERTIES,
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    buffers: FILL_PATTERN_BUFFERS,
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
};

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
const LINE_PROPS: ProgramBuffer = ProgramBuffer::uniform::<LineEvaluatedPropsUBO>("props", 0);

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
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    buffers: &[line_drawable::<LineGradientDrawableUBO>(), LINE_PROPS],
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
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    buffers: &[line_drawable::<LinePatternDrawableUBO>(), line_tile_props::<LinePatternTilePropsUBO>(), LINE_PROPS],
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
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    buffers: &[line_drawable::<LineSDFDrawableUBO>(), line_tile_props::<LineSDFTilePropsUBO>(), LINE_PROPS],
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
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    buffers: &[line_drawable::<LineSDFDrawableUBO>(), line_tile_props::<LineSDFTilePropsUBO>(), LINE_PROPS],
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
    textures: &[IMAGE, ProgramTexture::new("image_dash")],
    properties: &[
        Property::new("blur"),
        Property::new("opacity"),
        Property::new("gapwidth"),
        Property::new("offset"),
        Property::new("width"),
        Property::new("floorwidth"),
        Property::new("dasharray"),
    ],
};

/// Where a vertex samples the program's textures, in their texels.
const TEXTURE_POS: Attribute = Attribute::new("texture_pos", Short2);

pub static RASTER: Program = Program {
    name: "RasterShader",
    buffers: &[
        ProgramBuffer::storage::<RasterDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<RasterEvaluatedPropsUBO>("props", 0),
    ],
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image0"), ProgramTexture::new("image1")],
    properties: &[],
};

/// The fill extrusions' buffers. The uniform block is at slot 1, which
/// leaves slot 0 for the pattern's tiles.
const FILL_EXTRUSION_DRAWABLE: ProgramBuffer =
    ProgramBuffer::storage::<FillExtrusionDrawableUBO>("drawableVector", Stages::Vertex);
const FILL_EXTRUSION_PROPS: ProgramBuffer = ProgramBuffer::uniform::<FillExtrusionPropsUBO>("props", 1);
const FILL_EXTRUSION_BUFFERS: &[ProgramBuffer] = &[FILL_EXTRUSION_DRAWABLE, FILL_EXTRUSION_PROPS];
const FILL_EXTRUSION_PATTERN_BUFFERS: &[ProgramBuffer] = &[
    FILL_EXTRUSION_DRAWABLE,
    ProgramBuffer::storage::<FillExtrusionTilePropsUBO>("tilePropsVector", Stages::Both),
    FILL_EXTRUSION_PROPS,
];
const FILL_EXTRUSION_DECIMALS_ED: Attribute = Attribute::new("decimals_ed", UShort2);
const FILL_EXTRUSION_BASE: Attribute = Attribute::new("base", Float2);
const FILL_EXTRUSION_COLOR: Attribute = Attribute::new("color", Float4);
const FILL_EXTRUSION_HEIGHT: Attribute = Attribute::new("height", Float2);
const FILL_EXTRUSION_PATTERN_FROM: Attribute = Attribute::new("pattern_from", UShort4);
const FILL_EXTRUSION_PATTERN_TO: Attribute = Attribute::new("pattern_to", UShort4);
/// A wall instance's outline vertex, and the outline vertex after it.
const FILL_EXTRUSION_OUTLINE_POS: Attribute = Attribute::new("outline_pos", Short2);
const FILL_EXTRUSION_NEXT_OUTLINE_POS: Attribute = Attribute::new("next_outline_pos", Short2);
const FILL_EXTRUSION_NEXT_DECIMALS_ED: Attribute = Attribute::new("next_decimals_ed", UShort2);
const FILL_EXTRUSION_PROPERTIES: &[Property] =
    &[Property::new("base"), Property::new("color"), Property::new("height")];
const FILL_EXTRUSION_PATTERN_PROPERTIES: &[Property] =
    &[Property::new("base"), Property::new("height"), Property::new("pattern_from"), Property::new("pattern_to")];

pub static FILL_EXTRUSION: Program = Program {
    name: "FillExtrusionShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
};

pub static FILL_EXTRUSION_INSTANCED: Program = Program {
    name: "FillExtrusionInstancedShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] with every attribute per
/// vertex, for walls whose quads are in the vertex buffer.
pub static FILL_EXTRUSION_WALL: Program = Program {
    name: "FillExtrusionWallShader",
    buffers: FILL_EXTRUSION_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_COLOR,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
};

/// The walls of [`FILL_EXTRUSION_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its wall's outline vertices and paint values
/// from storage buffers, by its index.
pub static FILL_EXTRUSION_PULLED: Program = Program {
    name: "FillExtrusionPulledShader",
    buffers: &[
        FILL_EXTRUSION_BUFFERS[0],
        FILL_EXTRUSION_BUFFERS[1],
        ProgramBuffer::vertices("wallVertices", 0),
        ProgramBuffer::vertices("wallPaint", 1),
        ProgramBuffer::vertices("wallHeights", 2),
    ],
    projection: Projection::PerDraw,
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: FILL_EXTRUSION_PROPERTIES,
};

pub static FILL_EXTRUSION_PATTERN: Program = Program {
    name: "FillExtrusionPatternShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
};

pub static FILL_EXTRUSION_PATTERN_INSTANCED: Program = Program {
    name: "FillExtrusionPatternInstancedShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
};

/// [`FILL_EXTRUSION_WALL`] of the pattern walls.
pub static FILL_EXTRUSION_PATTERN_WALL: Program = Program {
    name: "FillExtrusionPatternWallShader",
    buffers: FILL_EXTRUSION_PATTERN_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        POS,
        FILL_EXTRUSION_OUTLINE_POS,
        FILL_EXTRUSION_DECIMALS_ED,
        FILL_EXTRUSION_BASE,
        FILL_EXTRUSION_HEIGHT,
        FILL_EXTRUSION_PATTERN_FROM,
        FILL_EXTRUSION_PATTERN_TO,
        FILL_EXTRUSION_NEXT_OUTLINE_POS,
        FILL_EXTRUSION_NEXT_DECIMALS_ED,
        GROUND_HEIGHT,
    ],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: FILL_EXTRUSION_PATTERN_PROPERTIES,
};

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    buffers: &[
        ProgramBuffer::storage::<HeatmapDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<HeatmapEvaluatedPropsUBO>("props", 0),
    ],
    projection: Projection::PerDraw,
    attributes: &[POS, Attribute::new("weight", Float2), Attribute::new("radius", Float2), GROUND_HEIGHT],
    instance_attributes: &[],
    textures: &[],
    properties: &[Property::new("weight"), Property::new("radius")],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    buffers: &[ProgramBuffer::uniform::<HeatmapTexturePropsUBO>("ubo", 0)],
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("color_ramp")],
    properties: &[],
};

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    buffers: &[
        ProgramBuffer::storage::<HillshadeDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::storage::<HillshadeTilePropsUBO>("tilePropsVector", Stages::Fragment),
        ProgramBuffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 0),
    ],
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    buffers: &[
        ProgramBuffer::draw_uniform::<HillshadePrepareDrawableUBO>("drawable", Stages::Vertex),
        ProgramBuffer::draw_uniform::<HillshadePrepareTilePropsUBO>("tileProps", Stages::Both),
    ],
    projection: Projection::None,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    buffers: &[
        ProgramBuffer::storage::<ColorReliefDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::storage::<ColorReliefTilePropsUBO>("tilePropsVector", Stages::Both),
        ProgramBuffer::uniform::<ColorReliefEvaluatedPropsUBO>("props", 1),
    ],
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::unfilterable("elevation_stops"), ProgramTexture::new("color_stops")],
    properties: &[],
};

const SYMBOL_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::storage::<SymbolDrawableUBO>("drawableVector", Stages::Vertex),
    ProgramBuffer::storage::<SymbolTilePropsUBO>("tilePropsVector", Stages::Fragment),
    ProgramBuffer::uniform::<SymbolEvaluatedPropsUBO>("props", 0),
];
const SYMBOL_POS_OFFSET: Attribute = Attribute::new("pos_offset", Short4);
const SYMBOL_DATA: Attribute = Attribute::new("data", UShort4);
const SYMBOL_PIXEL_OFFSET: Attribute = Attribute::new("pixeloffset", Short4);
const SYMBOL_PROJECTED_POS: Attribute = Attribute::new("projected_pos", Float4);
const SYMBOL_FADE_OPACITY: Attribute = Attribute::new("fade_opacity", Float);
const SYMBOL_OPACITY: Attribute = Attribute::new("opacity", Float);
const SYMBOL_FILL_COLOR: Attribute = Attribute::new("fill_color", Float4);
const SYMBOL_HALO_COLOR: Attribute = Attribute::new("halo_color", Float4);
const SYMBOL_HALO_WIDTH: Attribute = Attribute::new("halo_width", Float);
const SYMBOL_HALO_BLUR: Attribute = Attribute::new("halo_blur", Float);
/// The quad's origin in its atlas (`SymbolLayoutVertex::a4`).
const SYMBOL_TEX_ORIGIN: Attribute = Attribute::new("tex_origin", UShort2);
/// The symbol's `symbol-height-offset` (`SymbolLayoutVertex::a5`).
const SYMBOL_HEIGHT_OFFSET: Attribute = Attribute::new("height_offset", Float);
const SYMBOL_SDF_PROPERTIES: &[Property] = &[
    Property::new("opacity"),
    Property::new("fill_color"),
    Property::new("halo_color"),
    Property::new("halo_width"),
    Property::new("halo_blur"),
];
/// The glyphs' or icons' atlas.
const SYMBOL_TEXTURE: ProgramTexture = ProgramTexture::new("texture");

pub static SYMBOL_ICON: Program = Program {
    name: "SymbolIconShader",
    buffers: SYMBOL_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PIXEL_OFFSET,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_OPACITY,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: &[Property::new("opacity")],
};

pub static SYMBOL_SDF: Program = Program {
    name: "SymbolSDFShader",
    buffers: SYMBOL_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PIXEL_OFFSET,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_FILL_COLOR,
        SYMBOL_HALO_COLOR,
        SYMBOL_OPACITY,
        SYMBOL_HALO_WIDTH,
        SYMBOL_HALO_BLUR,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::UNUSED, GROUND_DEPTH],
    properties: SYMBOL_SDF_PROPERTIES,
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "SymbolTextAndIconShader",
    buffers: SYMBOL_BUFFERS,
    projection: Projection::PerDraw,
    attributes: &[
        SYMBOL_POS_OFFSET,
        SYMBOL_DATA,
        SYMBOL_PROJECTED_POS,
        SYMBOL_FADE_OPACITY,
        SYMBOL_FILL_COLOR,
        SYMBOL_HALO_COLOR,
        SYMBOL_OPACITY,
        SYMBOL_HALO_WIDTH,
        SYMBOL_HALO_BLUR,
        SYMBOL_TEX_ORIGIN,
        SYMBOL_HEIGHT_OFFSET,
    ],
    instance_attributes: &[],
    textures: &[SYMBOL_TEXTURE, ProgramTexture::new("texture_icon"), GROUND_DEPTH],
    properties: SYMBOL_SDF_PROPERTIES,
};

/// The collision boxes' and circles' buffers, each of a draw's own.
const COLLISION_BUFFERS: &[ProgramBuffer] = &[
    ProgramBuffer::draw_uniform::<CollisionDrawableUBO>("drawable", Stages::Vertex),
    ProgramBuffer::draw_uniform::<CollisionTilePropsUBO>("tile_props", Stages::Both),
];
const COLLISION_ANCHOR_POS: Attribute = Attribute::new("anchor_pos", Short2);
const COLLISION_EXTRUDE: Attribute = Attribute::new("extrude", Short2);
const COLLISION_PLACED: Attribute = Attribute::new("placed", UShort2);

pub static COLLISION_BOX: Program = Program {
    name: "CollisionBoxShader",
    buffers: COLLISION_BUFFERS,
    projection: Projection::Own,
    attributes: &[
        POS,
        COLLISION_ANCHOR_POS,
        COLLISION_EXTRUDE,
        COLLISION_PLACED,
        Attribute::new("shift", Float2),
        // The symbol's `symbol-height-offset` (`CollisionBoxLayoutVertex::a4`).
        Attribute::new("height_offset", Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "CollisionCircleShader",
    buffers: COLLISION_BUFFERS,
    projection: Projection::Own,
    attributes: &[POS, COLLISION_ANCHOR_POS, COLLISION_EXTRUDE, COLLISION_PLACED],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static GLOBE_DEPTH: Program = Program {
    name: "GlobeDepthShader",
    buffers: &[],
    projection: Projection::PerDraw,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static SKY: Program = Program {
    name: "SkyShader",
    buffers: &[ProgramBuffer::uniform::<SkyPropsUBO>("sky", 0)],
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static ATMOSPHERE: Program = Program {
    name: "AtmosphereShader",
    buffers: &[ProgramBuffer::uniform::<AtmospherePropsUBO>("atmosphere", 0)],
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    buffers: &[ProgramBuffer::uniform::<LayerCompositeUBO>("props", 0)],
    projection: Projection::None,
    attributes: &[POS],
    instance_attributes: &[],
    textures: &[IMAGE, ProgramTexture::new("backdrop")],
    properties: &[],
};

/// The DEM: RGBA8 texels behind a one-texel border, which the vertex stage
/// reads texel by texel, without a sampler. It is at slot 2, which the
/// sources name, so that a program that reads the ground keeps slots 0 and 1
/// for its own textures.
const GROUND_DEM: ProgramTexture = ProgramTexture::unfilterable("ground_dem");

const TERRAIN_DRAWABLE: ProgramBuffer = ProgramBuffer::storage::<TerrainDrawableUBO>("drawableVector", Stages::Vertex);
/// How the terrain's surface stands, and its fog.
const TERRAIN_SURFACE: ProgramBuffer = ProgramBuffer::uniform::<TerrainSurfaceUBO>("terrain", 0);

/// The terrain's surface: a grid over a tile, raised by its DEM, with the
/// tile's drape over it, which the fragment stage samples.
pub static TERRAIN: Program = Program {
    name: "TerrainShader",
    buffers: &[
        TERRAIN_DRAWABLE,
        ProgramBuffer::storage::<HillshadeTilePropsUBO>("lightVector", Stages::Fragment),
        TERRAIN_SURFACE,
        ProgramBuffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 1),
    ],
    projection: Projection::PerDraw,
    // A grid vertex: x and y in tile units, whether it is a skirt's lower
    // edge, and 0.
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::new("drape"), GROUND_DEM],
    properties: &[],
};

/// The programs with variants that stand on terrain
/// ([`ProjectionVariant::stands`](crate::projection_variant::ProjectionVariant::stands)).
pub static STANDING: &[&Program] = &[
    &CIRCLE,
    &HEATMAP,
    &FILL_EXTRUSION,
    &FILL_EXTRUSION_INSTANCED,
    &FILL_EXTRUSION_WALL,
    &FILL_EXTRUSION_PULLED,
    &FILL_EXTRUSION_PATTERN,
    &FILL_EXTRUSION_PATTERN_INSTANCED,
    &FILL_EXTRUSION_PATTERN_WALL,
    &SYMBOL_ICON,
    &SYMBOL_SDF,
    &SYMBOL_TEXT_AND_ICON,
];

/// The ground's depth: the terrain's surface, as [`TERRAIN`] raises it,
/// writing each pixel's eye distance, which the layers that stand on
/// terrain read to hide behind the ground in front of them.
pub static TERRAIN_DEPTH: Program = Program {
    name: "TerrainDepthShader",
    buffers: &[TERRAIN_DRAWABLE, TERRAIN_SURFACE],
    projection: Projection::PerDraw,
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
};

/// For the backend conformance cases: quads whose vertex stage reads their
/// color from a texture, by texel. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static VERTEX_TEXEL: Program = Program {
    name: "VertexTexelShader",
    buffers: &[],
    projection: Projection::None,
    attributes: &[Attribute::new("pos_texel", Short4)],
    instance_attributes: &[],
    textures: &[IMAGE],
    properties: &[],
};

/// For the backend conformance cases: probes whose vertex stage reads a
/// height by the terrain's height rule, and whose fragment stage writes its
/// bits. The renderer doesn't draw with it.
#[cfg(feature = "gpu-conformance")]
pub static TERRAIN_PROBE: Program = Program {
    name: "TerrainProbeShader",
    buffers: &[TERRAIN_SURFACE],
    projection: Projection::None,
    attributes: &[
        // Where a probe draws, in clip x and y, then where it reads the
        // height, in tile units.
        Attribute::new("pos", Float4),
        // Where the probe reads its DEM: the scale and offset that map the
        // tile's uv to the DEM's (`uv * scale + offset`), then the DEM's
        // width in pixels, 0 for flat ground.
        Attribute::new("dem_coords", Float4),
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
};
