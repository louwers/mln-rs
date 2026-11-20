//! The built-in programs: the engine's own (`CLIPPING_MASK`, `SKY`,
//! `ATMOSPHERE`, `GLOBE_DEPTH`, `LAYER_COMPOSITE` and `TILE_DEBUG`), whose
//! sources each backend has, and the built-in layer types'.

use crate::gfx_types::AttributeDataType;
use crate::gfx_types::AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};
use crate::shaders::program::{Program, Property};
use crate::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE,
    ID_CIRCLE_POS_VERTEX_ATTRIBUTE, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE,
    ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE,
    ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, ID_COLOR_RELIEF_IMAGE_TEXTURE,
    ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_FILL_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE,
    ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE,
    ID_HEATMAP_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
    ID_HILLSHADE_IMAGE_TEXTURE, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASH_TEXTURE,
    ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_IMAGE_TEXTURE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE, ID_RASTER_IMAGE0_TEXTURE,
    ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
    ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};

const fn property(name: &'static str, attribute: usize) -> Property {
    Property { name, attribute }
}

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    attributes: &[(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_BACKGROUND_IMAGE_TEXTURE],
    properties: &[],
};

pub static CIRCLE: Program = Program {
    name: "CircleShader",
    attributes: &[
        (ID_CIRCLE_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        property("radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        property("blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        property("stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        property("stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        property("stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
};

pub static CLIPPING_MASK: Program = Program {
    name: "ClippingMaskProgram",
    attributes: &[(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static FILL: Program = Program {
    name: "FillShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE: Program = Program {
    name: "FillOutlineShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_PATTERN: Program = Program {
    name: "FillPatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN: Program = Program {
    name: "FillOutlinePatternShader",
    attributes: &[
        (ID_FILL_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_INSTANCED: Program = Program {
    name: "FillOutlineInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

/// The outlines of [`FILL_OUTLINE_INSTANCED`] as quads without vertex
/// attributes: a vertex reads its segment's ends and paint values from
/// storage buffers, by its index.
pub static FILL_OUTLINE_PULLED: Program = Program {
    name: "FillOutlinePulledShader",
    attributes: &[],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    ],
};

pub static FILL_OUTLINE_PATTERN_INSTANCED: Program = Program {
    name: "FillOutlinePatternInstancedShader",
    attributes: &[(ID_FILL_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[
        (ID_FILL_OUTLINE_POS_ATTRIBUTE, Short2),
        (ID_FILL_OUTLINE_END_ATTRIBUTE, Short2),
        (ID_FILL_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
        (ID_FILL_OPACITY_VERTEX_ATTRIBUTE, Float2),
    ],
    textures: &[ID_FILL_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        property("opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

const LINE_ATTRIBUTES: &[(usize, AttributeDataType)] = &[
    (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
    (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
    (ID_LINE_COLOR_VERTEX_ATTRIBUTE, Float4),
    (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
    (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
];

const LINE_PROPERTIES: &[Property] = &[
    property("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];

pub static LINE: Program = Program {
    name: "LineShader",
    attributes: LINE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
    properties: LINE_PROPERTIES,
};

pub static LINE_GRADIENT: Program = Program {
    name: "LineGradientShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_PATTERN: Program = Program {
    name: "LinePatternShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, UShort4),
        (ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, UShort4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_SDF: Program = Program {
    name: "LineSDFShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_COLOR_VERTEX_ATTRIBUTE, Float4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, Float4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE],
    properties: &[
        property("color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        property("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
};

pub static LINE_GRADIENT_SDF: Program = Program {
    name: "LineGradientSDFShader",
    attributes: &[
        (ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2),
        (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4),
        (ID_LINE_BLUR_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OPACITY_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_OFFSET_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, Float2),
        (ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, Float4),
    ],
    instance_attributes: &[],
    textures: &[ID_LINE_IMAGE_TEXTURE, ID_LINE_DASH_TEXTURE],
    properties: &[
        property("blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        property("opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        property("gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        property("offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        property("width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        property("floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        property("dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
};

pub static RASTER: Program = Program {
    name: "RasterShader",
    attributes: &[(ID_RASTER_POS_VERTEX_ATTRIBUTE, Short2), (ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE],
    properties: &[],
};

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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
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
        property("base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        property("height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        property("pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        property("pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP: Program = Program {
    name: "HeatmapShader",
    attributes: &[
        (ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, Float2),
        (ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, Float2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[
        property("weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        property("radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
};

pub static HEATMAP_TEXTURE: Program = Program {
    name: "HeatmapTextureShader",
    attributes: &[(ID_HEATMAP_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_COLOR_RAMP_TEXTURE],
    properties: &[],
};

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    attributes: &[(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, Short2), (ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_HILLSHADE_IMAGE_TEXTURE],
    properties: &[],
};

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    attributes: &[
        (ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2),
    ],
    instance_attributes: &[],
    textures: &[
        ID_COLOR_RELIEF_IMAGE_TEXTURE,
        ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
        ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE,
    ],
    properties: &[],
};

pub static SYMBOL_ICON: Program = Program {
    name: "SymbolIconShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE)],
};

pub static SYMBOL_SDF: Program = Program {
    name: "SymbolSDFShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_HALO_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_BLUR_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE],
    properties: &[
        property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        property("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        property("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        property("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        property("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
};

pub static SYMBOL_TEXT_AND_ICON: Program = Program {
    name: "SymbolTextAndIconShader",
    attributes: &[
        (ID_SYMBOL_POS_OFFSET_ATTRIBUTE, Short4),
        (ID_SYMBOL_DATA_ATTRIBUTE, UShort4),
        (ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, Float3),
        (ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_HALO_COLOR_ATTRIBUTE, Float4),
        (ID_SYMBOL_OPACITY_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, Float),
        (ID_SYMBOL_HALO_BLUR_ATTRIBUTE, Float),
        (ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, UShort2),
        (ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_IMAGE_ICON_TEXTURE],
    properties: &[
        property("opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        property("fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        property("halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        property("halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        property("halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    ],
};

pub static COLLISION_BOX: Program = Program {
    name: "CollisionBoxShader",
    attributes: &[
        (ID_COLLISION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, UShort2),
        (ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, Float2),
        (ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, Float),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static COLLISION_CIRCLE: Program = Program {
    name: "CollisionCircleShader",
    attributes: &[
        (ID_COLLISION_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, Short2),
        (ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, UShort2),
    ],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static GLOBE_DEPTH: Program = Program {
    name: "GlobeDepthShader",
    attributes: &[(ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static SKY: Program = Program {
    name: "SkyShader",
    attributes: &[(ID_SKY_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

pub static ATMOSPHERE: Program = Program {
    name: "AtmosphereShader",
    attributes: &[(ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};

/// A layer drawn into a texture, composited onto the frame.
pub static LAYER_COMPOSITE: Program = Program {
    name: "LayerCompositeShader",
    attributes: &[(ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE],
    properties: &[],
};

/// The engine's debug overlays of tiles: their borders and their text, drawn
/// as lines whose color, width and other properties are uniforms
/// ([`tile_debug_ubo`](crate::shaders::tile_debug_ubo)).
pub static TILE_DEBUG: Program = Program {
    name: "TileDebugShader",
    attributes: &[(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, Short2), (ID_LINE_DATA_VERTEX_ATTRIBUTE, UByte4)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
};
