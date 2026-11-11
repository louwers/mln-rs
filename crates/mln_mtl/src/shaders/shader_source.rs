// Ports the Metal `ShaderSource<BuiltIn, Backend::Type::Metal>`
// specialisations: the MSL strings come from include/mln/shaders/mtl/*.hpp
// through the `msl_sources` module; the attribute/texture tables from
// src/mln/shaders/mtl/*.cpp are below.
//
// The tables describe the vertex layout the port's renderer produces, which
// is C++'s with MLN_USE_SYMBOL_INSTANCING and MLN_USE_FILL_EXTRUSION_INSTANCING
// off: the fill-extrusion and symbol tables are the non-instanced ones, plus
// the attributes the port adds (`normal2d`, `tex_origin`). The vertex buffers
// follow the shader's UBOs: the layout vertices at `<layer>UBOCount + 0`, then
// the symbols' dynamic and opacity vertices, then the interleaved data-driven
// vertices.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::shaders::shader_source::BuiltIn;

use super::msl_sources;

/// The MSL members of a `ShaderSource<BuiltIn::X, gfx::Backend::Type::Metal>`
/// specialisation, as extracted.
///
/// A shader's library compiles from `msl_sources::PRELUDE + prelude + source`.
#[derive(Debug)]
pub struct MslShaderSource {
    /// The `BuiltIn` enumerator name.
    pub built_in: &'static str,
    pub name: &'static str,
    pub vertex_main_function: &'static str,
    pub fragment_main_function: &'static str,
    /// The shader's own prelude constant (its UBO ids and structs), "" if none.
    pub prelude: &'static str,
    pub source: &'static str,
}

/// `shaders::AttributeInfo` (include/mln/shaders/mtl/shader_program.hpp):
/// the vertex attribute `id` is `[[attribute(index)]]`, read from vertex
/// buffer `[[buffer(buffer_index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct AttributeInfo {
    pub index: usize,
    pub data_type: AttributeDataType,
    pub buffer_index: usize,
    pub id: usize,
}

/// `shaders::TextureInfo`: texture `id` binds to `[[texture(index)]]` and its
/// sampler to `[[sampler(index)]]`.
#[derive(Clone, Copy, Debug)]
pub struct TextureInfo {
    pub index: usize,
    pub id: usize,
}

/// A complete specialisation: MSL plus the `attributes`,
/// `instanceAttributes` and `textures` tables.
#[derive(Debug)]
pub struct ShaderSource {
    pub msl: &'static MslShaderSource,
    pub name: &'static str,
    pub attributes: &'static [AttributeInfo],
    pub instance_attributes: &'static [AttributeInfo],
    pub textures: &'static [TextureInfo],
}

// The `<layer>UBOCount` enumerators of include/mln/shaders/shader_defines.hpp
// (packed, as on Metal) that shader_defines.rs doesn't name.
const ATMOSPHERE_UBO_COUNT: usize = ATMOSPHERE_LAYER_UBO_COUNT;
const CIRCLE_UBO_COUNT: usize = CIRCLE_LAYER_UBO_COUNT;
const CLIPPING_MASK_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const COLOR_RELIEF_UBO_COUNT: usize = COLOR_RELIEF_LAYER_UBO_COUNT;
const DEBUG_UBO_COUNT: usize = ID_DEBUG_UBO + 1;
const FILL_UBO_COUNT: usize = FILL_LAYER_UBO_COUNT;
const FILL_EXTRUSION_UBO_COUNT: usize = FILL_EXTRUSION_LAYER_UBO_COUNT;
/// `globeDepthUBOCount` (include/mln/shaders/mtl/globe_depth.hpp).
const GLOBE_DEPTH_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const HEATMAP_UBO_COUNT: usize = HEATMAP_LAYER_UBO_COUNT;
const HEATMAP_TEXTURE_UBO_COUNT: usize = ID_HEATMAP_TEXTURE_PROPS_UBO + 1;
const HILLSHADE_PREPARE_UBO_COUNT: usize = ID_HILLSHADE_PREPARE_TILE_PROPS_UBO + 1;
const HILLSHADE_UBO_COUNT: usize = HILLSHADE_LAYER_UBO_COUNT;
const LINE_UBO_COUNT: usize = LINE_LAYER_UBO_COUNT;
const RASTER_UBO_COUNT: usize = RASTER_LAYER_UBO_COUNT;
const SKY_UBO_COUNT: usize = SKY_LAYER_UBO_COUNT;
const SYMBOL_UBO_COUNT: usize = SYMBOL_LAYER_UBO_COUNT;

const fn attr(index: usize, data_type: AttributeDataType, buffer_index: usize, id: usize) -> AttributeInfo {
    AttributeInfo { index, data_type, buffer_index, id }
}

const fn tex(index: usize, id: usize) -> TextureInfo {
    TextureInfo { index, id }
}

use AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};

macro_rules! shader_source {
    ($name:ident, $str:literal, $attrs:expr, $textures:expr) => {
        static $name: ShaderSource = ShaderSource {
            msl: &msl_sources::$name,
            name: $str,
            attributes: $attrs,
            instance_attributes: &[],
            textures: $textures,
        };
    };
}

// src/mln/shaders/mtl/background.cpp
const BACKGROUND_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, BACKGROUND_UBO_COUNT + 0, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)];
shader_source!(BACKGROUND_SHADER, "BackgroundShader", BACKGROUND_ATTRIBUTES, &[]);
shader_source!(
    BACKGROUND_PATTERN_SHADER,
    "BackgroundPatternShader",
    BACKGROUND_ATTRIBUTES,
    &[tex(0, ID_BACKGROUND_IMAGE_TEXTURE)]
);

// src/mln/shaders/mtl/circle.cpp
const CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, CIRCLE_UBO_COUNT + 0, ID_CIRCLE_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
    attr(3, Float2, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float4, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
    attr(6, Float2, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, Float2, CIRCLE_UBO_COUNT + 1, ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
];
shader_source!(CIRCLE_SHADER, "CircleShader", CIRCLE_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/clipping_mask.cpp
const CLIPPING_MASK_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, CLIPPING_MASK_UBO_COUNT + 0, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
shader_source!(CLIPPING_MASK_PROGRAM, "ClippingMaskProgram", CLIPPING_MASK_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/collision.cpp
const COLLISION_BOX_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, COLLISION_UBO_COUNT + 1, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    attr(4, Float2, COLLISION_UBO_COUNT + 1, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
];
const COLLISION_CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, COLLISION_UBO_COUNT + 0, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, COLLISION_UBO_COUNT + 1, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];
shader_source!(COLLISION_BOX_SHADER, "CollisionBoxShader", COLLISION_BOX_ATTRIBUTES, &[]);
shader_source!(COLLISION_CIRCLE_SHADER, "CollisionCircleShader", COLLISION_CIRCLE_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/color_relief.cpp
const COLOR_RELIEF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, COLOR_RELIEF_UBO_COUNT + 0, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, COLOR_RELIEF_UBO_COUNT + 0, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const COLOR_RELIEF_TEXTURES: &[TextureInfo] = &[
    tex(0, ID_COLOR_RELIEF_IMAGE_TEXTURE),
    tex(1, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE),
    tex(2, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE),
];
shader_source!(COLOR_RELIEF_SHADER, "ColorReliefShader", COLOR_RELIEF_ATTRIBUTES, COLOR_RELIEF_TEXTURES);

// src/mln/shaders/mtl/debug.cpp
const DEBUG_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, DEBUG_UBO_COUNT + 0, ID_DEBUG_POS_VERTEX_ATTRIBUTE)];
shader_source!(DEBUG_SHADER, "DebugShader", DEBUG_ATTRIBUTES, &[tex(0, ID_DEBUG_OVERLAY_TEXTURE)]);

// src/mln/shaders/mtl/fill.cpp
const FILL_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_UBO_COUNT + 0, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_OUTLINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_UBO_COUNT + 0, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_UBO_COUNT + 0, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, FILL_UBO_COUNT + 1, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(2, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(3, UShort4, FILL_UBO_COUNT + 1, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    attr(4, Float2, FILL_UBO_COUNT + 1, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_TEXTURES: &[TextureInfo] = &[tex(0, ID_FILL_IMAGE_TEXTURE)];
const FILL_OUTLINE_TRIANGULATED_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, FILL_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
];
shader_source!(FILL_SHADER, "FillShader", FILL_ATTRIBUTES, &[]);
shader_source!(FILL_OUTLINE_SHADER, "FillOutlineShader", FILL_OUTLINE_ATTRIBUTES, &[]);
shader_source!(FILL_PATTERN_SHADER, "FillPatternShader", FILL_PATTERN_ATTRIBUTES, FILL_PATTERN_TEXTURES);
shader_source!(FILL_OUTLINE_PATTERN_SHADER, "FillOutlinePatternShader", FILL_PATTERN_ATTRIBUTES, FILL_PATTERN_TEXTURES);
shader_source!(
    FILL_OUTLINE_TRIANGULATED_SHADER,
    "FillOutlineTriangulatedShader",
    FILL_OUTLINE_TRIANGULATED_ATTRIBUTES,
    &[]
);

// src/mln/shaders/mtl/fill_extrusion.cpp, with the non-instanced layout
// vertex's `normal2d` (`!MLN_USE_FILL_EXTRUSION_INSTANCING`, as WebGPU).
const FILL_EXTRUSION_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(2, Short2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_NORMAL_2D_VERTEX_ATTRIBUTE),
    // Data driven
    attr(3, Float4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    attr(4, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(5, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(2, Short2, FILL_EXTRUSION_UBO_COUNT + 0, ID_FILL_EXTRUSION_NORMAL_2D_VERTEX_ATTRIBUTE),
    // Data driven
    attr(3, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(4, Float2, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    attr(5, UShort4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(6, UShort4, FILL_EXTRUSION_UBO_COUNT + 1, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
shader_source!(FILL_EXTRUSION_SHADER, "FillExtrusionShader", FILL_EXTRUSION_ATTRIBUTES, &[]);
shader_source!(
    FILL_EXTRUSION_PATTERN_SHADER,
    "FillExtrusionPatternShader",
    FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    &[tex(0, ID_FILL_EXTRUSION_IMAGE_TEXTURE)]
);

// src/mln/shaders/mtl/globe_depth.cpp
const GLOBE_DEPTH_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, GLOBE_DEPTH_UBO_COUNT + 0, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
shader_source!(GLOBE_DEPTH_SHADER, "GlobeDepthShader", GLOBE_DEPTH_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/heatmap.cpp
const HEATMAP_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, HEATMAP_UBO_COUNT + 0, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float2, HEATMAP_UBO_COUNT + 1, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    attr(2, Float2, HEATMAP_UBO_COUNT + 1, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
shader_source!(HEATMAP_SHADER, "HeatmapShader", HEATMAP_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/heatmap_texture.cpp
const HEATMAP_TEXTURE_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, HEATMAP_TEXTURE_UBO_COUNT + 0, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    HEATMAP_TEXTURE_SHADER,
    "HeatmapTextureShader",
    HEATMAP_TEXTURE_ATTRIBUTES,
    &[tex(0, ID_HEATMAP_IMAGE_TEXTURE), tex(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)]
);

// src/mln/shaders/mtl/hillshade.cpp, hillshade_prepare.cpp
const HILLSHADE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, HILLSHADE_UBO_COUNT + 0, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, HILLSHADE_UBO_COUNT + 0, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const HILLSHADE_PREPARE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, HILLSHADE_PREPARE_UBO_COUNT + 0, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, HILLSHADE_PREPARE_UBO_COUNT + 0, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const HILLSHADE_TEXTURES: &[TextureInfo] = &[tex(0, ID_HILLSHADE_IMAGE_TEXTURE)];
shader_source!(HILLSHADE_SHADER, "HillshadeShader", HILLSHADE_ATTRIBUTES, HILLSHADE_TEXTURES);
shader_source!(HILLSHADE_PREPARE_SHADER, "HillshadePrepareShader", HILLSHADE_PREPARE_ATTRIBUTES, HILLSHADE_TEXTURES);

// src/mln/shaders/mtl/line.cpp
const LINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, LINE_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, LINE_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float4, LINE_UBO_COUNT + 1, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, LINE_UBO_COUNT + 1, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, LINE_UBO_COUNT + 1, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float2, LINE_UBO_COUNT + 1, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(6, Float2, LINE_UBO_COUNT + 1, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(7, Float2, LINE_UBO_COUNT + 1, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, LINE_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, LINE_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, LINE_UBO_COUNT + 1, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, LINE_UBO_COUNT + 1, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, LINE_UBO_COUNT + 1, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, LINE_UBO_COUNT + 1, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, LINE_UBO_COUNT + 1, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, LINE_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, LINE_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, LINE_UBO_COUNT + 1, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, LINE_UBO_COUNT + 1, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, LINE_UBO_COUNT + 1, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, LINE_UBO_COUNT + 1, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, LINE_UBO_COUNT + 1, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, UShort4, LINE_UBO_COUNT + 1, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(8, UShort4, LINE_UBO_COUNT + 1, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const LINE_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, LINE_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, LINE_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float4, LINE_UBO_COUNT + 1, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, LINE_UBO_COUNT + 1, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, LINE_UBO_COUNT + 1, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float2, LINE_UBO_COUNT + 1, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(6, Float2, LINE_UBO_COUNT + 1, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(7, Float2, LINE_UBO_COUNT + 1, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(8, Float2, LINE_UBO_COUNT + 1, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, LINE_UBO_COUNT + 0, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, LINE_UBO_COUNT + 0, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, LINE_UBO_COUNT + 1, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, LINE_UBO_COUNT + 1, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, LINE_UBO_COUNT + 1, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, LINE_UBO_COUNT + 1, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, LINE_UBO_COUNT + 1, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, Float2, LINE_UBO_COUNT + 1, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_IMAGE_TEXTURES: &[TextureInfo] = &[tex(0, ID_LINE_IMAGE_TEXTURE)];
shader_source!(LINE_SHADER, "LineShader", LINE_ATTRIBUTES, &[]);
shader_source!(LINE_GRADIENT_SHADER, "LineGradientShader", LINE_GRADIENT_ATTRIBUTES, LINE_IMAGE_TEXTURES);
shader_source!(LINE_PATTERN_SHADER, "LinePatternShader", LINE_PATTERN_ATTRIBUTES, LINE_IMAGE_TEXTURES);
shader_source!(LINE_SDF_SHADER, "LineSDFShader", LINE_SDF_ATTRIBUTES, LINE_IMAGE_TEXTURES);
shader_source!(
    LINE_GRADIENT_SDF_SHADER,
    "LineGradientSDFShader",
    LINE_GRADIENT_SDF_ATTRIBUTES,
    &[tex(0, ID_LINE_IMAGE_TEXTURE), tex(1, ID_LINE_DASH_TEXTURE)]
);

// src/mln/shaders/mtl/raster.cpp
const RASTER_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, RASTER_UBO_COUNT + 0, ID_RASTER_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, RASTER_UBO_COUNT + 0, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
shader_source!(
    RASTER_SHADER,
    "RasterShader",
    RASTER_ATTRIBUTES,
    &[tex(0, ID_RASTER_IMAGE0_TEXTURE), tex(1, ID_RASTER_IMAGE1_TEXTURE)]
);

// src/mln/shaders/mtl/sky.cpp
const SKY_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, SKY_UBO_COUNT + 0, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ATMOSPHERE_UBO_COUNT + 0, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
shader_source!(SKY_SHADER, "SkyShader", SKY_ATTRIBUTES, &[]);
shader_source!(ATMOSPHERE_SHADER, "AtmosphereShader", ATMOSPHERE_ATTRIBUTES, &[]);

// src/mln/shaders/mtl/symbol.cpp before the symbol instancing
// (`!MLN_USE_SYMBOL_INSTANCING`): the layout vertex (`SymbolLayoutVertex`,
// with the port's `tex_origin`), the dynamic vertex (`projected_pos`), the
// opacity vertex (`fade_opacity`) and the interleaved data-driven vertex, in
// four buffers.
const SYMBOL_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    // Layout
    attr(6, UShort2, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(6, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(7, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(8, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(9, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(10, UShort2, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_DATA_ATTRIBUTE),
    // Dynamic
    attr(2, Float3, SYMBOL_UBO_COUNT + 1, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(3, Float, SYMBOL_UBO_COUNT + 2, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(4, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(5, Float4, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(6, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(7, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(8, Float2, SYMBOL_UBO_COUNT + 3, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(9, UShort2, SYMBOL_UBO_COUNT + 0, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_TEXTURES: &[TextureInfo] = &[tex(0, ID_SYMBOL_IMAGE_TEXTURE)];
shader_source!(SYMBOL_ICON_SHADER, "SymbolIconShader", SYMBOL_ICON_ATTRIBUTES, SYMBOL_TEXTURES);
shader_source!(SYMBOL_SDF_SHADER, "SymbolSDFShader", SYMBOL_SDF_ATTRIBUTES, SYMBOL_TEXTURES);
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    "SymbolTextAndIconShader",
    SYMBOL_TEXT_AND_ICON_ATTRIBUTES,
    &[tex(0, ID_SYMBOL_IMAGE_TEXTURE), tex(1, ID_SYMBOL_IMAGE_ICON_TEXTURE)]
);

/// `ShaderSource<shader_id, Metal>`, or `None` for the shaders the port's
/// renderer doesn't draw with: the instanced fill extrusions, the location
/// indicator and the wide vector.
pub fn shader_source(shader_id: BuiltIn) -> Option<&'static ShaderSource> {
    match shader_id {
        BuiltIn::BackgroundShader => Some(&BACKGROUND_SHADER),
        BuiltIn::BackgroundPatternShader => Some(&BACKGROUND_PATTERN_SHADER),
        BuiltIn::CircleShader => Some(&CIRCLE_SHADER),
        BuiltIn::ClippingMaskProgram => Some(&CLIPPING_MASK_PROGRAM),
        BuiltIn::CollisionBoxShader => Some(&COLLISION_BOX_SHADER),
        BuiltIn::CollisionCircleShader => Some(&COLLISION_CIRCLE_SHADER),
        BuiltIn::ColorReliefShader => Some(&COLOR_RELIEF_SHADER),
        BuiltIn::DebugShader => Some(&DEBUG_SHADER),
        BuiltIn::FillShader => Some(&FILL_SHADER),
        BuiltIn::FillOutlineShader => Some(&FILL_OUTLINE_SHADER),
        BuiltIn::FillPatternShader => Some(&FILL_PATTERN_SHADER),
        BuiltIn::FillOutlinePatternShader => Some(&FILL_OUTLINE_PATTERN_SHADER),
        BuiltIn::FillOutlineTriangulatedShader => Some(&FILL_OUTLINE_TRIANGULATED_SHADER),
        BuiltIn::FillExtrusionShader => Some(&FILL_EXTRUSION_SHADER),
        BuiltIn::FillExtrusionPatternShader => Some(&FILL_EXTRUSION_PATTERN_SHADER),
        BuiltIn::HeatmapShader => Some(&HEATMAP_SHADER),
        BuiltIn::HeatmapTextureShader => Some(&HEATMAP_TEXTURE_SHADER),
        BuiltIn::HillshadeShader => Some(&HILLSHADE_SHADER),
        BuiltIn::HillshadePrepareShader => Some(&HILLSHADE_PREPARE_SHADER),
        BuiltIn::LineShader => Some(&LINE_SHADER),
        BuiltIn::LineGradientShader => Some(&LINE_GRADIENT_SHADER),
        BuiltIn::LinePatternShader => Some(&LINE_PATTERN_SHADER),
        BuiltIn::LineSDFShader => Some(&LINE_SDF_SHADER),
        BuiltIn::LineGradientSDFShader => Some(&LINE_GRADIENT_SDF_SHADER),
        BuiltIn::RasterShader => Some(&RASTER_SHADER),
        BuiltIn::SymbolIconShader => Some(&SYMBOL_ICON_SHADER),
        BuiltIn::SymbolSDFShader => Some(&SYMBOL_SDF_SHADER),
        BuiltIn::SymbolTextAndIconShader => Some(&SYMBOL_TEXT_AND_ICON_SHADER),
        BuiltIn::SkyShader => Some(&SKY_SHADER),
        BuiltIn::AtmosphereShader => Some(&ATMOSPHERE_SHADER),
        BuiltIn::GlobeDepthShader => Some(&GLOBE_DEPTH_SHADER),
        _ => None,
    }
}
