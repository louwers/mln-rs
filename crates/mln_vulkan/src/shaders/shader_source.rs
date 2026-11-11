// Ports the Vulkan `ShaderSource<BuiltIn, Backend::Type::Vulkan>`
// specialisations: the GLSL strings come from include/mln/shaders/vulkan/*.hpp
// through the `glsl_sources` module; the attribute and texture tables from
// src/mln/shaders/vulkan/*.cpp are below.
//
// The tables describe the vertex layout the port's renderer produces, which
// is C++'s with MLN_USE_FILL_EXTRUSION_INSTANCING on and
// MLN_USE_SYMBOL_INSTANCING off: the symbol tables are the non-instanced ones,
// plus the attribute the port adds (`tex_origin`), and the instanced fill
// extrusions' have the next outline vertex as instance attributes of its own.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::shader_defines::*;
use mln_gfx::shaders::shader_source::BuiltIn;

use super::glsl_sources;

/// The GLSL members of a `ShaderSource<BuiltIn::X, gfx::Backend::Type::Vulkan>`
/// specialisation.
///
/// A stage compiles from the stage's prelude (`glsl_sources::PRELUDE_VERTEX`
/// or `PRELUDE_FRAGMENT`), `prelude` and the stage's source
/// (`compiler::compose`).
#[derive(Debug)]
pub struct GlslShaderSource {
    /// The `BuiltIn` enumerator name.
    pub built_in: &'static str,
    pub name: &'static str,
    /// The shader's own prelude constant (its buffer bindings), "" if none.
    pub prelude: &'static str,
    pub vertex: &'static str,
    pub fragment: &'static str,
}

/// `shaders::AttributeInfo` (include/mln/shaders/vulkan/shader_program.hpp):
/// the vertex or instance attribute `id` is the input at
/// `layout(location = index)`. C++'s `ubo`, the storage buffer an instanced
/// attribute is also read through, is not ported: every attribute is a vertex
/// input.
#[derive(Clone, Copy, Debug)]
pub struct AttributeInfo {
    pub index: usize,
    pub data_type: AttributeDataType,
    pub id: usize,
}

/// `shaders::TextureInfo`: texture `id` binds in the image set at
/// `layout::image_texture_binding(index)`, and its sampler at
/// `layout::image_sampler_binding(index)`.
#[derive(Clone, Copy, Debug)]
pub struct TextureInfo {
    pub index: usize,
    pub id: usize,
}

/// A complete specialisation: GLSL plus the `attributes`,
/// `instanceAttributes` and `textures` tables.
#[derive(Debug)]
pub struct ShaderSource {
    pub glsl: &'static GlslShaderSource,
    pub name: &'static str,
    pub attributes: &'static [AttributeInfo],
    pub instance_attributes: &'static [AttributeInfo],
    pub textures: &'static [TextureInfo],
}

const fn attr(index: usize, data_type: AttributeDataType, id: usize) -> AttributeInfo {
    AttributeInfo { index, data_type, id }
}

const fn tex(index: usize, id: usize) -> TextureInfo {
    TextureInfo { index, id }
}

use AttributeDataType::{Float, Float2, Float3, Float4, Short2, Short4, UByte4, UShort2, UShort4};

macro_rules! shader_source {
    ($name:ident, $str:literal, $attrs:expr, $textures:expr) => {
        shader_source!($name, $str, $attrs, &[], $textures);
    };
    ($name:ident, $str:literal, $attrs:expr, $instance_attrs:expr, $textures:expr) => {
        static $name: ShaderSource = ShaderSource {
            glsl: &glsl_sources::$name,
            name: $str,
            attributes: $attrs,
            instance_attributes: $instance_attrs,
            textures: $textures,
        };
    };
}

// src/mln/shaders/vulkan/background.cpp
const BACKGROUND_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)];
shader_source!(BACKGROUND_SHADER, "BackgroundShader", BACKGROUND_ATTRIBUTES, &[]);
shader_source!(
    BACKGROUND_PATTERN_SHADER,
    "BackgroundPatternShader",
    BACKGROUND_ATTRIBUTES,
    &[tex(0, ID_BACKGROUND_IMAGE_TEXTURE)]
);

// src/mln/shaders/vulkan/circle.cpp
const CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_CIRCLE_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float4, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
];
shader_source!(CIRCLE_SHADER, "CircleShader", CIRCLE_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/clipping_mask.cpp. The globe variant also reads
// the tile's projection vectors at the locations 1 to 3, which the context
// binds as instance attributes itself.
const CLIPPING_MASK_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
shader_source!(CLIPPING_MASK_PROGRAM, "ClippingMaskProgram", CLIPPING_MASK_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/collision.cpp
const COLLISION_BOX_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
];
const COLLISION_CIRCLE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_COLLISION_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
    attr(2, Short2, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
    // Dynamic
    attr(3, UShort2, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
];
shader_source!(COLLISION_BOX_SHADER, "CollisionBoxShader", COLLISION_BOX_ATTRIBUTES, &[]);
shader_source!(COLLISION_CIRCLE_SHADER, "CollisionCircleShader", COLLISION_CIRCLE_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/color_relief.cpp. The GLSL works the texture
// position out from the position, and has no input for it.
const COLOR_RELIEF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE),
    attr(1, Short2, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE),
];
const COLOR_RELIEF_TEXTURES: &[TextureInfo] = &[
    tex(0, ID_COLOR_RELIEF_IMAGE_TEXTURE),
    tex(1, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE),
    tex(2, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE),
];
shader_source!(COLOR_RELIEF_SHADER, "ColorReliefShader", COLOR_RELIEF_ATTRIBUTES, COLOR_RELIEF_TEXTURES);

// src/mln/shaders/vulkan/debug.cpp
const DEBUG_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_DEBUG_POS_VERTEX_ATTRIBUTE)];
shader_source!(DEBUG_SHADER, "DebugShader", DEBUG_ATTRIBUTES, &[tex(0, ID_DEBUG_OVERLAY_TEXTURE)]);

// src/mln/shaders/vulkan/fill.cpp
const FILL_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_OUTLINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float4, ID_FILL_COLOR_VERTEX_ATTRIBUTE),
    attr(2, UShort4, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(3, UShort4, ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
];
const FILL_PATTERN_TEXTURES: &[TextureInfo] = &[tex(0, ID_FILL_IMAGE_TEXTURE)];
const FILL_OUTLINE_TRIANGULATED_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE), attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE)];
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

// src/mln/shaders/vulkan/fill_extrusion.cpp. An instance of the instanced
// shaders is an outline vertex; the port adds the vertex after it, which C++
// reads from the instances' storage buffer.
const FILL_EXTRUSION_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    attr(2, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
const FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    attr(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    attr(3, Float4, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    // The next outline vertex
    attr(6, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    attr(7, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
    attr(1, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    attr(2, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    attr(4, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(5, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(1, Short2, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
    attr(2, UShort2, ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
    // Data driven
    attr(3, Float2, ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
    attr(5, UShort4, ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(6, UShort4, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    // The next outline vertex
    attr(7, Short2, ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
    attr(8, UShort2, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
];
const FILL_EXTRUSION_PATTERN_TEXTURES: &[TextureInfo] = &[tex(0, ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
shader_source!(FILL_EXTRUSION_SHADER, "FillExtrusionShader", FILL_EXTRUSION_ATTRIBUTES, &[]);
shader_source!(
    FILL_EXTRUSION_INSTANCED_SHADER,
    "FillExtrusionInstancedShader",
    FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    FILL_EXTRUSION_INSTANCED_INSTANCE_ATTRIBUTES,
    &[]
);
shader_source!(
    FILL_EXTRUSION_PATTERN_SHADER,
    "FillExtrusionPatternShader",
    FILL_EXTRUSION_PATTERN_ATTRIBUTES,
    FILL_EXTRUSION_PATTERN_TEXTURES
);
shader_source!(
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    "FillExtrusionPatternInstancedShader",
    FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    FILL_EXTRUSION_PATTERN_INSTANCED_INSTANCE_ATTRIBUTES,
    FILL_EXTRUSION_PATTERN_TEXTURES
);

// src/mln/shaders/vulkan/globe_depth.cpp
const GLOBE_DEPTH_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
shader_source!(GLOBE_DEPTH_SHADER, "GlobeDepthShader", GLOBE_DEPTH_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/heatmap.cpp
const HEATMAP_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
    // Data driven
    attr(1, Float2, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
    attr(2, Float2, ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
];
shader_source!(HEATMAP_SHADER, "HeatmapShader", HEATMAP_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/heatmap_texture.cpp
const HEATMAP_TEXTURE_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_HEATMAP_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    HEATMAP_TEXTURE_SHADER,
    "HeatmapTextureShader",
    HEATMAP_TEXTURE_ATTRIBUTES,
    &[tex(0, ID_HEATMAP_IMAGE_TEXTURE), tex(1, ID_HEATMAP_COLOR_RAMP_TEXTURE)]
);

// src/mln/shaders/vulkan/hillshade.cpp, hillshade_prepare.cpp
const HILLSHADE_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE), attr(1, Short2, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE)];
const HILLSHADE_PREPARE_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_HILLSHADE_POS_VERTEX_ATTRIBUTE), attr(1, Short2, ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE)];
const HILLSHADE_TEXTURES: &[TextureInfo] = &[tex(0, ID_HILLSHADE_IMAGE_TEXTURE)];
shader_source!(HILLSHADE_SHADER, "HillshadeShader", HILLSHADE_ATTRIBUTES, HILLSHADE_TEXTURES);
shader_source!(HILLSHADE_PREPARE_SHADER, "HillshadePrepareShader", HILLSHADE_PREPARE_ATTRIBUTES, HILLSHADE_TEXTURES);

// src/mln/shaders/vulkan/line.cpp
const LINE_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float4, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_PATTERN_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, UShort4, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
    attr(8, UShort4, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
];
const LINE_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float4, ID_LINE_COLOR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(8, Float2, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
];
const LINE_GRADIENT_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    attr(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
    // Data driven
    attr(2, Float2, ID_LINE_BLUR_VERTEX_ATTRIBUTE),
    attr(3, Float2, ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
    attr(4, Float2, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
    attr(5, Float2, ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
    attr(6, Float2, ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    attr(7, Float2, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
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

// src/mln/shaders/vulkan/raster.cpp
const RASTER_ATTRIBUTES: &[AttributeInfo] =
    &[attr(0, Short2, ID_RASTER_POS_VERTEX_ATTRIBUTE), attr(1, Short2, ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    RASTER_SHADER,
    "RasterShader",
    RASTER_ATTRIBUTES,
    &[tex(0, ID_RASTER_IMAGE0_TEXTURE), tex(1, ID_RASTER_IMAGE1_TEXTURE)]
);

// src/mln/shaders/vulkan/sky.cpp
const SKY_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[AttributeInfo] = &[attr(0, Short2, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
shader_source!(SKY_SHADER, "SkyShader", SKY_ATTRIBUTES, &[]);
shader_source!(ATMOSPHERE_SHADER, "AtmosphereShader", ATMOSPHERE_ATTRIBUTES, &[]);

// src/mln/shaders/vulkan/symbol.cpp before the symbol instancing
// (`!MLN_USE_SYMBOL_INSTANCING`): the layout vertex (`SymbolLayoutVertex`,
// with the port's `tex_origin`), the dynamic vertex (`projected_pos`), the
// opacity vertex (`fade_opacity`) and the interleaved data-driven vertex, in
// four buffers.
const SYMBOL_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    // Layout
    attr(6, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_SDF_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    attr(2, Short4, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
    // Dynamic
    attr(3, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(4, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(5, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(6, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(7, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(8, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(9, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(10, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
];
const SYMBOL_TEXT_AND_ICON_ATTRIBUTES: &[AttributeInfo] = &[
    attr(0, Short4, ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
    attr(1, UShort4, ID_SYMBOL_DATA_ATTRIBUTE),
    // Dynamic
    attr(2, Float3, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
    // Opacity
    attr(3, Float, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
    // Data driven
    attr(4, Float4, ID_SYMBOL_COLOR_ATTRIBUTE),
    attr(5, Float4, ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
    attr(6, Float2, ID_SYMBOL_OPACITY_ATTRIBUTE),
    attr(7, Float2, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
    attr(8, Float2, ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
    // Layout
    attr(9, UShort2, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
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

/// `ShaderSource<shader_id, Vulkan>`, or `None` for the shaders the port's
/// renderer doesn't draw with: the location indicator and the wide vector.
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
        BuiltIn::FillExtrusionInstancedShader => Some(&FILL_EXTRUSION_INSTANCED_SHADER),
        BuiltIn::FillExtrusionPatternShader => Some(&FILL_EXTRUSION_PATTERN_SHADER),
        BuiltIn::FillExtrusionPatternInstancedShader => Some(&FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
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
