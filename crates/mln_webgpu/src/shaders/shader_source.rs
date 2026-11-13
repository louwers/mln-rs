use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE,
    ID_CIRCLE_POS_VERTEX_ATTRIBUTE, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE,
    ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE,
    ID_COLOR_RELIEF_IMAGE_TEXTURE, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE,
    ID_DEBUG_OVERLAY_TEXTURE, ID_DEBUG_POS_VERTEX_ATTRIBUTE, ID_FILL_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_POS_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_COLOR_RAMP_TEXTURE,
    ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE, ID_HILLSHADE_IMAGE_TEXTURE, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE,
    ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE,
    ID_LINE_DASH_TEXTURE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_IMAGE_TEXTURE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE, ID_RASTER_IMAGE0_TEXTURE,
    ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
    ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE,
    ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE,
    ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
};
use mln_gfx::shaders::shader_source::BuiltIn;

use super::{fill_extrusion_wgsl, wgsl_sources};

/// The WGSL members of a `ShaderSource` specialisation, as extracted.
#[derive(Debug)]
pub(crate) struct WgslShaderSource {
    /// The `BuiltIn` enumerator name.
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests name the shaders they check by them"))]
    pub(crate) built_in: &'static str,
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests name the shaders they check by them"))]
    pub(crate) name: &'static str,
    pub(crate) vertex: Option<&'static str>,
    pub(crate) fragment: Option<&'static str>,
    pub(crate) prelude: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AttributeInfo {
    pub(crate) index: usize,
    pub(crate) data_type: AttributeDataType,
    pub(crate) id: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TextureInfo {
    pub(crate) index: usize,
    pub(crate) id: usize,
}

/// A complete specialisation: WGSL plus the `attributes`,
/// `instanceAttributes` and `textures` tables.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) wgsl: &'static WgslShaderSource,
    pub(crate) name: &'static str,
    pub(crate) attributes: &'static [AttributeInfo],
    pub(crate) instance_attributes: &'static [AttributeInfo],
    pub(crate) textures: &'static [TextureInfo],
}

// src/mln/shaders/webgpu/background.cpp
const BACKGROUND_ATTRIBUTES: &[AttributeInfo] =
    &[AttributeInfo { index: 3, data_type: AttributeDataType::Short2, id: ID_BACKGROUND_POS_VERTEX_ATTRIBUTE }];
const BACKGROUND_PATTERN_TEXTURES: &[TextureInfo] = &[TextureInfo { index: 0, id: ID_BACKGROUND_IMAGE_TEXTURE }];

// src/mln/shaders/webgpu/clipping_mask.cpp
const CLIPPING_MASK_ATTRIBUTES: &[AttributeInfo] =
    &[AttributeInfo { index: 2, data_type: AttributeDataType::Short2, id: ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE }];

static BACKGROUND_SHADER: ShaderSource = ShaderSource {
    wgsl: &wgsl_sources::BACKGROUND_SHADER,
    name: "BackgroundShader",
    attributes: BACKGROUND_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

static BACKGROUND_PATTERN_SHADER: ShaderSource = ShaderSource {
    wgsl: &wgsl_sources::BACKGROUND_PATTERN_SHADER,
    name: "BackgroundPatternShader",
    attributes: BACKGROUND_ATTRIBUTES,
    instance_attributes: &[],
    textures: BACKGROUND_PATTERN_TEXTURES,
};

static CLIPPING_MASK_PROGRAM: ShaderSource = ShaderSource {
    wgsl: &wgsl_sources::CLIPPING_MASK_PROGRAM,
    name: "ClippingMaskProgram",
    attributes: CLIPPING_MASK_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const fn attr(index: usize, data_type: AttributeDataType, id: usize) -> AttributeInfo {
    AttributeInfo { index, data_type, id }
}

const fn tex(index: usize, id: usize) -> TextureInfo {
    TextureInfo { index, id }
}

use AttributeDataType::{Float2, Float4, Short2, UByte4, UShort2, UShort4};

// src/mln/shaders/webgpu/circle.cpp
const CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_CIRCLE_POS_VERTEX_ATTRIBUTE),
    attr(1, Float4, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float4, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
];

// src/mln/shaders/webgpu/fill.cpp
const FILL_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    attr(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_OUTLINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    attr(1, Float4, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    attr(5, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(6, UShort4, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(7, UShort4, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_TEXTURES: &[TextureInfo] = &[tex(0, ID_FILL_IMAGE_TEXTURE)];
const FILL_OUTLINE_TRIANGULATED_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE), attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE)];

// src/mln/shaders/webgpu/line.cpp
const LINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(5, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    attr(6, Float4, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(9, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(10, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(11, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(5, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(9, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(10, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_IMAGE_TEXTURES: &[TextureInfo] = &[tex(0, ID_LINE_IMAGE_TEXTURE)];
const LINE_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(5, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(9, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(10, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(11, UShort4, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(12, UShort4, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const LINE_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(5, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    attr(6, Float4, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(9, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(10, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(11, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(12, Float2, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(4, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(5, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(9, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(10, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(11, Float2, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
];
// A texture at location L binds its sampler at L and the texture at L + 1,
// so the dash atlas sits at 2.
const LINE_GRADIENT_SDF_TEXTURES: &[TextureInfo] = &[tex(0, ID_LINE_IMAGE_TEXTURE), tex(2, ID_LINE_DASH_TEXTURE)];

macro_rules! shader_source {
    ($name:ident, $wgsl:ident, $str:literal, $attrs:expr, $textures:expr) => {
        static $name: ShaderSource = ShaderSource {
            wgsl: &wgsl_sources::$wgsl,
            name: $str,
            attributes: $attrs,
            instance_attributes: &[],
            textures: $textures,
        };
    };
}

shader_source!(CIRCLE_SHADER, CIRCLE_SHADER, "CircleShader", CIRCLE_ATTRIBUTES, &[]);
shader_source!(FILL_SHADER, FILL_SHADER, "FillShader", FILL_ATTRIBUTES, &[]);
shader_source!(FILL_OUTLINE_SHADER, FILL_OUTLINE_SHADER, "FillOutlineShader", FILL_OUTLINE_ATTRIBUTES, &[]);
shader_source!(
    FILL_PATTERN_SHADER,
    FILL_PATTERN_SHADER,
    "FillPatternShader",
    FILL_PATTERN_ATTRIBUTES,
    FILL_PATTERN_TEXTURES
);
shader_source!(
    FILL_OUTLINE_PATTERN_SHADER,
    FILL_OUTLINE_PATTERN_SHADER,
    "FillOutlinePatternShader",
    FILL_PATTERN_ATTRIBUTES,
    FILL_PATTERN_TEXTURES
);
shader_source!(
    FILL_OUTLINE_TRIANGULATED_SHADER,
    FILL_OUTLINE_TRIANGULATED_SHADER,
    "FillOutlineTriangulatedShader",
    FILL_OUTLINE_TRIANGULATED_ATTRIBUTES,
    &[]
);
shader_source!(LINE_SHADER, LINE_SHADER, "LineShader", LINE_ATTRIBUTES, &[]);
shader_source!(
    LINE_GRADIENT_SHADER,
    LINE_GRADIENT_SHADER,
    "LineGradientShader",
    LINE_GRADIENT_ATTRIBUTES,
    LINE_IMAGE_TEXTURES
);
shader_source!(
    LINE_PATTERN_SHADER,
    LINE_PATTERN_SHADER,
    "LinePatternShader",
    LINE_PATTERN_ATTRIBUTES,
    LINE_IMAGE_TEXTURES
);
shader_source!(LINE_SDF_SHADER, LINE_SDF_SHADER, "LineSDFShader", LINE_SDF_ATTRIBUTES, LINE_IMAGE_TEXTURES);
shader_source!(
    LINE_GRADIENT_SDF_SHADER,
    LINE_GRADIENT_SDF_SHADER,
    "LineGradientSDFShader",
    LINE_GRADIENT_SDF_ATTRIBUTES,
    LINE_GRADIENT_SDF_TEXTURES
);

// src/mln/shaders/webgpu/raster.cpp (conformance)
const RASTER_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_RASTER_POS_VERTEX_ATTRIBUTE), attr(1, Short2, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE)];
const RASTER_TEXTURES: &[TextureInfo] = &[tex(0, ID_RASTER_IMAGE0_TEXTURE), tex(1, ID_RASTER_IMAGE1_TEXTURE)];
shader_source!(RASTER_SHADER, RASTER_SHADER, "RasterShader", RASTER_ATTRIBUTES, RASTER_TEXTURES);

// dem-heatmap: src/mln/shaders/webgpu/heatmap.cpp
const HEATMAP_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    attr(1, Float2, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
shader_source!(HEATMAP_SHADER, HEATMAP_SHADER, "HeatmapShader", HEATMAP_ATTRIBUTES, &[]);
// src/mln/shaders/webgpu/heatmap_texture.cpp
const HEATMAP_TEXTURE_ATTRIBUTES: &[AttributeInfo] = &[attr(5, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
const HEATMAP_TEXTURE_TEXTURES: &[TextureInfo] =
    &[tex(0, ID_HEATMAP_IMAGE_TEXTURE), tex(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)];
shader_source!(
    HEATMAP_TEXTURE_SHADER,
    HEATMAP_TEXTURE_SHADER,
    "HeatmapTextureShader",
    HEATMAP_TEXTURE_ATTRIBUTES,
    HEATMAP_TEXTURE_TEXTURES
);
// src/mln/shaders/webgpu/hillshade.cpp, hillshade_prepare.cpp
const HILLSHADE_ATTRIBUTES: &[AttributeInfo] =
    &[attr(5, Short2, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE), attr(6, Short2, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE)];
const HILLSHADE_TEXTURES: &[TextureInfo] = &[tex(0, ID_HILLSHADE_IMAGE_TEXTURE)];
shader_source!(HILLSHADE_SHADER, HILLSHADE_SHADER, "HillshadeShader", HILLSHADE_ATTRIBUTES, HILLSHADE_TEXTURES);
shader_source!(
    HILLSHADE_PREPARE_SHADER,
    HILLSHADE_PREPARE_SHADER,
    "HillshadePrepareShader",
    HILLSHADE_ATTRIBUTES,
    HILLSHADE_TEXTURES
);
// src/mln/shaders/webgpu/color_relief.cpp
const COLOR_RELIEF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(5, Short2, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE),
    attr(6, Short2, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const COLOR_RELIEF_TEXTURES: &[TextureInfo] = &[
    tex(0, ID_COLOR_RELIEF_IMAGE_TEXTURE),
    tex(1, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE),
    tex(2, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE),
];
shader_source!(
    COLOR_RELIEF_SHADER,
    COLOR_RELIEF_SHADER,
    "ColorReliefShader",
    COLOR_RELIEF_ATTRIBUTES,
    COLOR_RELIEF_TEXTURES
);

// The fill extrusions' (the port's, fill_extrusion_wgsl.rs): the attribute
// locations of the roofs, and of the walls with their instances.
const FILL_EXTRUSION_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(2, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
const FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    attr(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(3, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    attr(4, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
    attr(5, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(2, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    attr(4, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(5, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    attr(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    attr(3, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    attr(4, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
    attr(5, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    attr(7, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(8, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_TEXTURES: &[TextureInfo] = &[tex(0, ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
static FILL_EXTRUSION_SHADER: ShaderSource = ShaderSource {
    wgsl: &fill_extrusion_wgsl::FILL_EXTRUSION_SHADER,
    name: "FillExtrusionShader",
    attributes: FILL_EXTRUSION_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};
static FILL_EXTRUSION_INSTANCED_SHADER: ShaderSource = ShaderSource {
    wgsl: &fill_extrusion_wgsl::FILL_EXTRUSION_INSTANCED_SHADER,
    name: "FillExtrusionInstancedShader",
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: &[],
};
static FILL_EXTRUSION_PATTERN_SHADER: ShaderSource = ShaderSource {
    wgsl: &fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_SHADER,
    name: "FillExtrusionPatternShader",
    attributes: FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};
static FILL_EXTRUSION_PATTERN_INSTANCED_SHADER: ShaderSource = ShaderSource {
    wgsl: &fill_extrusion_wgsl::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    name: "FillExtrusionPatternInstancedShader",
    attributes: FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    instance_attributes: FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

// src/mln/shaders/webgpu/symbol.cpp (symbols)
const SYMBOL_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(3, AttributeDataType::Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(4, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(5, AttributeDataType::Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    attr(6, AttributeDataType::Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    attr(7, AttributeDataType::Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    attr(8, AttributeDataType::Float, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(9, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_ICON_TEXTURES: &[TextureInfo] = &[tex(0, ID_SYMBOL_IMAGE_TEXTURE)];
const SYMBOL_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(3, AttributeDataType::Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(4, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(5, AttributeDataType::Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    attr(6, AttributeDataType::Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    attr(7, AttributeDataType::Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    attr(8, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(9, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(10, AttributeDataType::Float, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(11, AttributeDataType::Float, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(12, AttributeDataType::Float, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    attr(13, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_SDF_TEXTURES: &[TextureInfo] = &[tex(0, ID_SYMBOL_IMAGE_TEXTURE)];
const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(3, AttributeDataType::Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(4, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(5, AttributeDataType::Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    attr(6, AttributeDataType::Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    attr(7, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(8, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(9, AttributeDataType::Float, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(10, AttributeDataType::Float, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(11, AttributeDataType::Float, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    attr(12, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_TEXT_AND_ICON_TEXTURES: &[TextureInfo] =
    &[tex(0, ID_SYMBOL_IMAGE_TEXTURE), tex(2, ID_SYMBOL_IMAGE_ICON_TEXTURE)];
shader_source!(
    SYMBOL_ICON_SHADER,
    SYMBOL_ICON_SHADER,
    "SymbolIconShader",
    SYMBOL_ICON_ATTRIBUTES,
    SYMBOL_ICON_TEXTURES
);
shader_source!(SYMBOL_SDF_SHADER, SYMBOL_SDF_SHADER, "SymbolSDFShader", SYMBOL_SDF_ATTRIBUTES, SYMBOL_SDF_TEXTURES);
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    SYMBOL_TEXT_AND_ICON_SHADER,
    "SymbolTextAndIconShader",
    SYMBOL_TEXT_AND_ICON_ATTRIBUTES,
    SYMBOL_TEXT_AND_ICON_TEXTURES
);

// src/mln/shaders/webgpu/collision.cpp (symbols)
const COLLISION_BOX_ATTRIBUTES: &[AttributeInfo] = &[
    attr(3, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(4, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(5, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    attr(6, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
];
const COLLISION_CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(3, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(4, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(5, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    attr(6, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];
shader_source!(COLLISION_BOX_SHADER, COLLISION_BOX_SHADER, "CollisionBoxShader", COLLISION_BOX_ATTRIBUTES, &[]);
shader_source!(
    COLLISION_CIRCLE_SHADER,
    COLLISION_CIRCLE_SHADER,
    "CollisionCircleShader",
    COLLISION_CIRCLE_ATTRIBUTES,
    &[]
);

// layers: debug (src/mln/shaders/webgpu/debug.cpp)
const DEBUG_ATTRIBUTES: &[AttributeInfo] = &[attr(5, Short2, ID_DEBUG_POS_VERTEX_ATTRIBUTE)];
const DEBUG_TEXTURES: &[TextureInfo] = &[tex(0, ID_DEBUG_OVERLAY_TEXTURE)];
shader_source!(DEBUG_SHADER, DEBUG_SHADER, "DebugShader", DEBUG_ATTRIBUTES, DEBUG_TEXTURES);

// src/mln/shaders/webgpu/sky.cpp
const SKY_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_SKY_POS_VERTEX_ATTRIBUTE)];
shader_source!(SKY_SHADER, SKY_SHADER, "SkyShader", SKY_ATTRIBUTES, &[]);
const ATMOSPHERE_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
shader_source!(ATMOSPHERE_SHADER, ATMOSPHERE_SHADER, "AtmosphereShader", ATMOSPHERE_ATTRIBUTES, &[]);

// src/mln/shaders/webgpu/globe_depth.cpp
const GLOBE_DEPTH_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
shader_source!(GLOBE_DEPTH_SHADER, GLOBE_DEPTH_SHADER, "GlobeDepthShader", GLOBE_DEPTH_ATTRIBUTES, &[]);

/// The WebGPU source of `shader_id`, or `None` for the shaders without WGSL
/// yet.
#[must_use]
pub(crate) fn shader_source(shader_id: BuiltIn) -> Option<&'static ShaderSource> {
    match shader_id {
        BuiltIn::BackgroundShader => Some(&BACKGROUND_SHADER),
        BuiltIn::BackgroundPatternShader => Some(&BACKGROUND_PATTERN_SHADER),
        BuiltIn::CircleShader => Some(&CIRCLE_SHADER),
        BuiltIn::ClippingMaskProgram => Some(&CLIPPING_MASK_PROGRAM),
        BuiltIn::FillShader => Some(&FILL_SHADER),
        BuiltIn::FillOutlineShader => Some(&FILL_OUTLINE_SHADER),
        BuiltIn::FillPatternShader => Some(&FILL_PATTERN_SHADER),
        BuiltIn::FillOutlinePatternShader => Some(&FILL_OUTLINE_PATTERN_SHADER),
        BuiltIn::FillOutlineTriangulatedShader => Some(&FILL_OUTLINE_TRIANGULATED_SHADER),
        BuiltIn::LineShader => Some(&LINE_SHADER),
        BuiltIn::LineGradientShader => Some(&LINE_GRADIENT_SHADER),
        BuiltIn::LinePatternShader => Some(&LINE_PATTERN_SHADER),
        BuiltIn::LineSDFShader => Some(&LINE_SDF_SHADER),
        BuiltIn::LineGradientSDFShader => Some(&LINE_GRADIENT_SDF_SHADER),
        BuiltIn::RasterShader => Some(&RASTER_SHADER),
        BuiltIn::FillExtrusionShader => Some(&FILL_EXTRUSION_SHADER),
        BuiltIn::FillExtrusionInstancedShader => Some(&FILL_EXTRUSION_INSTANCED_SHADER),
        BuiltIn::FillExtrusionPatternShader => Some(&FILL_EXTRUSION_PATTERN_SHADER),
        BuiltIn::FillExtrusionPatternInstancedShader => Some(&FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
        // dem-heatmap
        BuiltIn::HeatmapShader => Some(&HEATMAP_SHADER),
        BuiltIn::HeatmapTextureShader => Some(&HEATMAP_TEXTURE_SHADER),
        BuiltIn::HillshadeShader => Some(&HILLSHADE_SHADER),
        BuiltIn::HillshadePrepareShader => Some(&HILLSHADE_PREPARE_SHADER),
        BuiltIn::ColorReliefShader => Some(&COLOR_RELIEF_SHADER),
        // symbols
        BuiltIn::SymbolIconShader => Some(&SYMBOL_ICON_SHADER),
        BuiltIn::SymbolSDFShader => Some(&SYMBOL_SDF_SHADER),
        BuiltIn::SymbolTextAndIconShader => Some(&SYMBOL_TEXT_AND_ICON_SHADER),
        BuiltIn::CollisionBoxShader => Some(&COLLISION_BOX_SHADER),
        BuiltIn::CollisionCircleShader => Some(&COLLISION_CIRCLE_SHADER),
        BuiltIn::DebugShader => Some(&DEBUG_SHADER), // layers: debug
        BuiltIn::GlobeDepthShader => Some(&GLOBE_DEPTH_SHADER),
        BuiltIn::SkyShader => Some(&SKY_SHADER),
        BuiltIn::AtmosphereShader => Some(&ATMOSPHERE_SHADER),
        _ => None,
    }
}

#[must_use]
pub(crate) fn prelude() -> &'static str {
    wgsl_sources::PRELUDE.prelude.expect("Prelude without prelude")
}
