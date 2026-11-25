// Each shader's GLSL and the tables of where its uniform blocks bind. A
// program's attribute `name` is the shader's `a_<name>`, and its texture
// `name` the sampler `u_<name>`, which samples the texture unit of its slot.
// - The ids are shader_defines.rs's, so a shader's uniform blocks bind where
//   the renderer puts their buffers.
// - A block's entry has the size of its UBO, which the compile test checks
//   against the program, and, where the renderer writes the block for every
//   drawable of a layer into one buffer, the size of that buffer's elements.
// - Every block a shader's GLSL declares has an entry, the prelude's
//   projection block too: WebGL2 rejects a draw whose program has a block
//   without a buffer, whether or not the shader reads it.

use std::mem::size_of;

use mln_gfx::shaders::background_layer_ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use mln_gfx::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use mln_gfx::shaders::color_relief_layer_ubo::{
    ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO,
};
use mln_gfx::shaders::fill_extrusion_layer_ubo::{
    FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO,
};
use mln_gfx::shaders::fill_layer_ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use mln_gfx::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use mln_gfx::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use mln_gfx::shaders::hillshade_layer_ubo::{HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO};
use mln_gfx::shaders::hillshade_prepare_layer_ubo::{HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use mln_gfx::shaders::layer_composite_ubo::LayerCompositeUBO;
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::line_layer_ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_PROPS_UBO, ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_PROPS_UBO, ID_CIRCLE_DRAWABLE_UBO,
    ID_CIRCLE_EVALUATED_PROPS_UBO, ID_COLLISION_DRAWABLE_UBO, ID_COLLISION_TILE_PROPS_UBO,
    ID_COLOR_RELIEF_DRAWABLE_UBO, ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, ID_COLOR_RELIEF_TILE_PROPS_UBO,
    ID_FILL_DRAWABLE_UBO, ID_FILL_EVALUATED_PROPS_UBO, ID_FILL_EXTRUSION_DRAWABLE_UBO, ID_FILL_EXTRUSION_PROPS_UBO,
    ID_FILL_EXTRUSION_TILE_PROPS_UBO, ID_FILL_TILE_PROPS_UBO, ID_GLOBAL_PAINT_PARAMS_UBO, ID_HEATMAP_DRAWABLE_UBO,
    ID_HEATMAP_EVALUATED_PROPS_UBO, ID_HEATMAP_TEXTURE_PROPS_UBO, ID_HILLSHADE_DRAWABLE_UBO,
    ID_HILLSHADE_EVALUATED_PROPS_UBO, ID_HILLSHADE_PREPARE_DRAWABLE_UBO, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
    ID_HILLSHADE_TILE_PROPS_UBO, ID_LAYER_COMPOSITE_UBO, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_TILE_PROPS_UBO, ID_PROJECTION_UBO, ID_RASTER_DRAWABLE_UBO, ID_RASTER_EVALUATED_PROPS_UBO, ID_SKY_PROPS_UBO,
    ID_SYMBOL_DRAWABLE_UBO, ID_SYMBOL_EVALUATED_PROPS_UBO, ID_SYMBOL_TILE_PROPS_UBO, ID_TERRAIN_DRAWABLE_UBO,
    ID_TERRAIN_LIGHT_TILE_UBO, ID_TERRAIN_LIGHT_UBO, ID_TERRAIN_PROPS_UBO,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use mln_gfx::shaders::terrain_ubo::{TerrainDrawableUBO, TerrainSurfaceUBO};

use super::glsl_sources;

/// A program's GLSL.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the prelude's source of that stage
/// and the shader's.
#[derive(Debug)]
pub(crate) struct GlslShaderSource {
    pub(crate) vertex: &'static str,
    pub(crate) fragment: &'static str,
}

/// The uniform block `name` binds to the binding point `id`, where the
/// buffer of that id is bound.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UniformBlockInfo {
    pub(crate) name: &'static str,
    pub(crate) id: usize,
    /// The size of the block's UBO.
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests check it against the block the shader declares"))]
    pub(crate) size: usize,
    /// The size of an element of the layer's buffer that holds this block for
    /// each of its drawables, or 0 if the renderer writes no such buffer.
    pub(crate) element_size: usize,
}

/// A program's OpenGL source: its GLSL, and the table of where its uniform
/// blocks bind.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) glsl: &'static GlslShaderSource,
    pub(crate) uniform_blocks: &'static [UniformBlockInfo],
}

/// A block whose UBO is a `T`.
const fn block<T>(name: &'static str, id: usize) -> UniformBlockInfo {
    UniformBlockInfo { name, id, size: size_of::<T>(), element_size: 0 }
}

/// A block whose UBO is a `T`, one in each `Element` of a layer's buffer.
const fn element<T, Element>(name: &'static str, id: usize) -> UniformBlockInfo {
    assert!(size_of::<T>() <= size_of::<Element>());
    UniformBlockInfo { name, id, size: size_of::<T>(), element_size: size_of::<Element>() }
}

macro_rules! shader_source {
    ($name:ident, $blocks:expr) => {
        static $name: ShaderSource = ShaderSource { glsl: &glsl_sources::$name, uniform_blocks: $blocks };
    };
}

// Clipping mask
shader_source!(CLIPPING_MASK_PROGRAM, &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)]);

// Background
shader_source!(
    BACKGROUND_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>(
            "BackgroundDrawableUBO",
            ID_BACKGROUND_DRAWABLE_UBO
        ),
        block::<BackgroundPropsUBO>("BackgroundPropsUBO", ID_BACKGROUND_PROPS_UBO),
    ]
);

// Background Pattern
shader_source!(
    BACKGROUND_PATTERN_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(
            "BackgroundPatternDrawableUBO",
            ID_BACKGROUND_DRAWABLE_UBO
        ),
        block::<BackgroundPatternPropsUBO>("BackgroundPatternPropsUBO", ID_BACKGROUND_PROPS_UBO),
    ]
);

// Sky
shader_source!(
    SKY_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<SkyPropsUBO>("SkyPropsUBO", ID_SKY_PROPS_UBO),
    ]
);

// Atmosphere
shader_source!(
    ATMOSPHERE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<AtmospherePropsUBO>("AtmospherePropsUBO", ID_ATMOSPHERE_PROPS_UBO),
    ]
);

// Circle
shader_source!(
    CIRCLE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<CircleDrawableUBO, CircleDrawableUBO>("CircleDrawableUBO", ID_CIRCLE_DRAWABLE_UBO),
        block::<CircleEvaluatedPropsUBO>("CircleEvaluatedPropsUBO", ID_CIRCLE_EVALUATED_PROPS_UBO),
    ]
);

// Collision Box
shader_source!(
    COLLISION_BOX_SHADER,
    &[
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        block::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ]
);

// Collision Circle
shader_source!(
    COLLISION_CIRCLE_SHADER,
    &[
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        block::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ]
);

// Globe depth
shader_source!(GLOBE_DEPTH_SHADER, &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),]);

// Layer composite
shader_source!(
    LAYER_COMPOSITE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<LayerCompositeUBO>("LayerCompositeUBO", ID_LAYER_COMPOSITE_UBO),
    ]
);

// Vertex texel
#[cfg(test)]
shader_source!(VERTEX_TEXEL_SHADER, &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)]);

// Terrain
shader_source!(
    TERRAIN_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<TerrainDrawableUBO, TerrainDrawableUBO>("TerrainDrawableUBO", ID_TERRAIN_DRAWABLE_UBO),
        block::<TerrainSurfaceUBO>("TerrainPropsUBO", ID_TERRAIN_PROPS_UBO),
        element::<HillshadeTilePropsUBO, HillshadeTilePropsUBO>("HillshadeTilePropsUBO", ID_TERRAIN_LIGHT_TILE_UBO),
        block::<HillshadeEvaluatedPropsUBO>("HillshadeEvaluatedPropsUBO", ID_TERRAIN_LIGHT_UBO),
    ]
);

// The ground's depth
shader_source!(
    TERRAIN_DEPTH_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<TerrainDrawableUBO, TerrainDrawableUBO>("TerrainDrawableUBO", ID_TERRAIN_DRAWABLE_UBO),
        block::<TerrainSurfaceUBO>("TerrainPropsUBO", ID_TERRAIN_PROPS_UBO),
    ]
);

// Terrain probe
#[cfg(test)]
shader_source!(
    TERRAIN_PROBE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<TerrainSurfaceUBO>("TerrainPropsUBO", ID_TERRAIN_PROPS_UBO),
    ]
);

// Fill
shader_source!(
    FILL_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<FillDrawableUBO, FillDrawableUnionUBO>("FillDrawableUBO", ID_FILL_DRAWABLE_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Outline
shader_source!(
    FILL_OUTLINE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillOutlineDrawableUBO, FillDrawableUnionUBO>("FillOutlineDrawableUBO", ID_FILL_DRAWABLE_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Pattern
shader_source!(
    FILL_PATTERN_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillPatternDrawableUBO, FillDrawableUnionUBO>("FillPatternDrawableUBO", ID_FILL_DRAWABLE_UBO),
        element::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>("FillPatternTilePropsUBO", ID_FILL_TILE_PROPS_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Outline Pattern
shader_source!(
    FILL_OUTLINE_PATTERN_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillPatternDrawableUBO, FillDrawableUnionUBO>("FillOutlinePatternDrawableUBO", ID_FILL_DRAWABLE_UBO),
        element::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>(
            "FillOutlinePatternTilePropsUBO",
            ID_FILL_TILE_PROPS_UBO
        ),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Outline Instanced: an instance of a quad per outline segment, where
// the context draws no 2 px lines
shader_source!(
    FILL_OUTLINE_INSTANCED_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillOutlineDrawableUBO, FillDrawableUnionUBO>("FillOutlineDrawableUBO", ID_FILL_DRAWABLE_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Outline Pattern Instanced
shader_source!(
    FILL_OUTLINE_PATTERN_INSTANCED_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillPatternDrawableUBO, FillDrawableUnionUBO>("FillOutlinePatternDrawableUBO", ID_FILL_DRAWABLE_UBO),
        element::<FillPatternTilePropsUBO, FillTilePropsUnionUBO>(
            "FillOutlinePatternTilePropsUBO",
            ID_FILL_TILE_PROPS_UBO
        ),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ]
);

// Fill Extrusion: the roofs
const FILL_EXTRUSION_BLOCKS: &[UniformBlockInfo] = &[
    element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
    element::<FillExtrusionDrawableUBO, FillExtrusionDrawableUBO>(
        "FillExtrusionDrawableUBO",
        ID_FILL_EXTRUSION_DRAWABLE_UBO,
    ),
    element::<FillExtrusionTilePropsUBO, FillExtrusionTilePropsUBO>(
        "FillExtrusionTilePropsUBO",
        ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ),
    block::<FillExtrusionPropsUBO>("FillExtrusionPropsUBO", ID_FILL_EXTRUSION_PROPS_UBO),
];
shader_source!(FILL_EXTRUSION_SHADER, FILL_EXTRUSION_BLOCKS);

// Fill Extrusion Instanced: the walls, an instance per outline vertex
shader_source!(FILL_EXTRUSION_INSTANCED_SHADER, FILL_EXTRUSION_BLOCKS);

// Fill Extrusion Wall: the instanced walls' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer
static FILL_EXTRUSION_WALL_SHADER: ShaderSource =
    ShaderSource { glsl: &glsl_sources::FILL_EXTRUSION_INSTANCED_SHADER, uniform_blocks: FILL_EXTRUSION_BLOCKS };

// Fill Extrusion Pattern: the roofs
const FILL_EXTRUSION_PATTERN_BLOCKS: &[UniformBlockInfo] = &[
    element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
    block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
    element::<FillExtrusionDrawableUBO, FillExtrusionDrawableUBO>(
        "FillExtrusionDrawableUBO",
        ID_FILL_EXTRUSION_DRAWABLE_UBO,
    ),
    element::<FillExtrusionTilePropsUBO, FillExtrusionTilePropsUBO>(
        "FillExtrusionTilePropsUBO",
        ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ),
    block::<FillExtrusionPropsUBO>("FillExtrusionPropsUBO", ID_FILL_EXTRUSION_PROPS_UBO),
];
shader_source!(FILL_EXTRUSION_PATTERN_SHADER, FILL_EXTRUSION_PATTERN_BLOCKS);

// Fill Extrusion Pattern Instanced: the walls
shader_source!(FILL_EXTRUSION_PATTERN_INSTANCED_SHADER, FILL_EXTRUSION_PATTERN_BLOCKS);

// Fill Extrusion Pattern Wall: as Fill Extrusion Wall
static FILL_EXTRUSION_PATTERN_WALL_SHADER: ShaderSource = ShaderSource {
    glsl: &glsl_sources::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_PATTERN_BLOCKS,
};

// Heatmap
shader_source!(
    HEATMAP_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<HeatmapDrawableUBO, HeatmapDrawableUBO>("HeatmapDrawableUBO", ID_HEATMAP_DRAWABLE_UBO),
        block::<HeatmapEvaluatedPropsUBO>("HeatmapEvaluatedPropsUBO", ID_HEATMAP_EVALUATED_PROPS_UBO),
    ]
);

// Heatmap Texture
shader_source!(
    HEATMAP_TEXTURE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        block::<HeatmapTexturePropsUBO>("HeatmapTexturePropsUBO", ID_HEATMAP_TEXTURE_PROPS_UBO),
    ]
);

// Hillshade Prepare
shader_source!(
    HILLSHADE_PREPARE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<HillshadePrepareDrawableUBO>("HillshadePrepareDrawableUBO", ID_HILLSHADE_PREPARE_DRAWABLE_UBO),
        block::<HillshadePrepareTilePropsUBO>("HillshadePrepareTilePropsUBO", ID_HILLSHADE_PREPARE_TILE_PROPS_UBO),
    ]
);

// Hillshade
shader_source!(
    HILLSHADE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<HillshadeDrawableUBO, HillshadeDrawableUBO>("HillshadeDrawableUBO", ID_HILLSHADE_DRAWABLE_UBO),
        element::<HillshadeTilePropsUBO, HillshadeTilePropsUBO>("HillshadeTilePropsUBO", ID_HILLSHADE_TILE_PROPS_UBO),
        block::<HillshadeEvaluatedPropsUBO>("HillshadeEvaluatedPropsUBO", ID_HILLSHADE_EVALUATED_PROPS_UBO),
    ]
);

// Color Relief
shader_source!(
    COLOR_RELIEF_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<ColorReliefDrawableUBO, ColorReliefDrawableUBO>(
            "ColorReliefDrawableUBO",
            ID_COLOR_RELIEF_DRAWABLE_UBO
        ),
        element::<ColorReliefTilePropsUBO, ColorReliefTilePropsUBO>(
            "ColorReliefTilePropsUBO",
            ID_COLOR_RELIEF_TILE_PROPS_UBO
        ),
        block::<ColorReliefEvaluatedPropsUBO>("ColorReliefEvaluatedPropsUBO", ID_COLOR_RELIEF_EVALUATED_PROPS_UBO),
    ]
);

// Line
shader_source!(
    LINE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LineDrawableUBO, LineDrawableUnionUBO>("LineDrawableUBO", ID_LINE_DRAWABLE_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ]
);

// Line Gradient
shader_source!(
    LINE_GRADIENT_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LineGradientDrawableUBO, LineDrawableUnionUBO>("LineGradientDrawableUBO", ID_LINE_DRAWABLE_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ]
);

// Line Pattern
shader_source!(
    LINE_PATTERN_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LinePatternDrawableUBO, LineDrawableUnionUBO>("LinePatternDrawableUBO", ID_LINE_DRAWABLE_UBO),
        element::<LinePatternTilePropsUBO, LineTilePropsUnionUBO>("LinePatternTilePropsUBO", ID_LINE_TILE_PROPS_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ]
);

// Line SDF
shader_source!(
    LINE_SDF_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LineSDFDrawableUBO, LineDrawableUnionUBO>("LineSDFDrawableUBO", ID_LINE_DRAWABLE_UBO),
        element::<LineSDFTilePropsUBO, LineTilePropsUnionUBO>("LineSDFTilePropsUBO", ID_LINE_TILE_PROPS_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ]
);

// Line gradient SDF
shader_source!(
    LINE_GRADIENT_SDF_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LineSDFDrawableUBO, LineDrawableUnionUBO>("LineSDFDrawableUBO", ID_LINE_DRAWABLE_UBO),
        element::<LineSDFTilePropsUBO, LineTilePropsUnionUBO>("LineSDFTilePropsUBO", ID_LINE_TILE_PROPS_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ]
);

// Raster
shader_source!(
    RASTER_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<RasterDrawableUBO, RasterDrawableUBO>("RasterDrawableUBO", ID_RASTER_DRAWABLE_UBO),
        block::<RasterEvaluatedPropsUBO>("RasterEvaluatedPropsUBO", ID_RASTER_EVALUATED_PROPS_UBO),
    ]
);

// Symbol Icon
shader_source!(
    SYMBOL_ICON_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        block::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ]
);

// Symbol SDF
shader_source!(
    SYMBOL_SDF_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        block::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ]
);

// Symbol Text & Icon
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        block::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ]
);

/// The source of each built-in program that has GLSL.
pub(crate) static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_OUTLINE, &FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
