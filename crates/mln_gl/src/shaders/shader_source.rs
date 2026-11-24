// Each shader's GLSL and the tables of where its uniform blocks, attributes
// and textures bind:
// - The ids are shader_defines.rs's, so a shader's uniform blocks bind where
//   the renderer puts their buffers.
// - A block's entry has the size of its UBO, which the compile test checks
//   against the program, and, where the renderer writes the block for every
//   drawable of a layer into one buffer, the size of that buffer's elements.
// - The fill extrusion shaders have the roofs' attributes, and the instanced
//   ones, which the renderer draws the walls with, the instances'.
// - RasterShader's second texture has an id of its own.
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
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_ATMOSPHERE_PROPS_UBO, ID_BACKGROUND_DRAWABLE_UBO,
    ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO,
    ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE, ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE, ID_CIRCLE_DRAWABLE_UBO,
    ID_CIRCLE_EVALUATED_PROPS_UBO, ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_POS_VERTEX_ATTRIBUTE,
    ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE,
    ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE, ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_DRAWABLE_UBO,
    ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE, ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE,
    ID_COLLISION_PLACED_VERTEX_ATTRIBUTE, ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE,
    ID_COLLISION_TILE_PROPS_UBO, ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_DRAWABLE_UBO,
    ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, ID_COLOR_RELIEF_IMAGE_TEXTURE,
    ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TILE_PROPS_UBO,
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_DRAWABLE_UBO, ID_FILL_EVALUATED_PROPS_UBO,
    ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE, ID_FILL_EXTRUSION_DRAWABLE_UBO, ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE,
    ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE, ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE,
    ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE, ID_FILL_EXTRUSION_PROPS_UBO, ID_FILL_EXTRUSION_TILE_PROPS_UBO,
    ID_FILL_IMAGE_TEXTURE, ID_FILL_OPACITY_VERTEX_ATTRIBUTE, ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE,
    ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE, ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE,
    ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE, ID_FILL_TILE_PROPS_UBO,
    ID_GLOBAL_PAINT_PARAMS_UBO, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_HEATMAP_COLOR_RAMP_TEXTURE,
    ID_HEATMAP_DRAWABLE_UBO, ID_HEATMAP_EVALUATED_PROPS_UBO, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
    ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_TEXTURE_PROPS_UBO, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE,
    ID_HILLSHADE_DRAWABLE_UBO, ID_HILLSHADE_EVALUATED_PROPS_UBO, ID_HILLSHADE_IMAGE_TEXTURE,
    ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_PREPARE_DRAWABLE_UBO, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
    ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_HILLSHADE_TILE_PROPS_UBO, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE,
    ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_UBO,
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASH_TEXTURE,
    ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE, ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO,
    ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
    ID_LINE_IMAGE_TEXTURE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE, ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
    ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE,
    ID_LINE_TILE_PROPS_UBO, ID_LINE_WIDTH_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO, ID_RASTER_DRAWABLE_UBO,
    ID_RASTER_EVALUATED_PROPS_UBO, ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SKY_PROPS_UBO, ID_SYMBOL_COLOR_ATTRIBUTE,
    ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_DRAWABLE_UBO, ID_SYMBOL_EVALUATED_PROPS_UBO, ID_SYMBOL_FADE_OPACITY_ATTRIBUTE,
    ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE, ID_SYMBOL_HALO_WIDTH_ATTRIBUTE,
    ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE, ID_SYMBOL_POS_OFFSET_ATTRIBUTE,
    ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE, ID_SYMBOL_TILE_PROPS_UBO,
};
#[cfg(test)]
use mln_gfx::shaders::shader_defines::{
    ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE,
    ID_TERRAIN_PROPS_UBO, ID_VERTEX_TEXEL_POS_TEXEL_VERTEX_ATTRIBUTE, ID_VERTEX_TEXEL_TEXTURE,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
#[cfg(test)]
use mln_gfx::shaders::terrain_ubo::TerrainPropsUBO;

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

/// The vertex attribute `name` reads attribute `id`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AttributeInfo {
    pub(crate) name: &'static str,
    pub(crate) id: usize,
}

/// The sampler `name` reads texture `id`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextureInfo {
    pub(crate) name: &'static str,
    pub(crate) id: usize,
}

/// A program's OpenGL source: its GLSL, and the tables of where its uniform
/// blocks, attributes and textures bind.
#[derive(Debug)]
pub(crate) struct ShaderSource {
    pub(crate) glsl: &'static GlslShaderSource,
    pub(crate) uniform_blocks: &'static [UniformBlockInfo],
    pub(crate) attributes: &'static [AttributeInfo],
    /// The attributes that step once an instance.
    pub(crate) instance_attributes: &'static [AttributeInfo],
    pub(crate) textures: &'static [TextureInfo],
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

const fn attr(name: &'static str, id: usize) -> AttributeInfo {
    AttributeInfo { name, id }
}

const fn tex(name: &'static str, id: usize) -> TextureInfo {
    TextureInfo { name, id }
}

macro_rules! shader_source {
    ($name:ident, $blocks:expr, $attrs:expr, $textures:expr) => {
        shader_source!($name, $blocks, $attrs, &[], $textures);
    };
    ($name:ident, $blocks:expr, $attrs:expr, $instance_attrs:expr, $textures:expr) => {
        static $name: ShaderSource = ShaderSource {
            glsl: &glsl_sources::$name,
            uniform_blocks: $blocks,
            attributes: $attrs,
            instance_attributes: $instance_attrs,
            textures: $textures,
        };
    };
}

// Clipping mask
shader_source!(
    CLIPPING_MASK_PROGRAM,
    &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)],
    &[attr("a_pos", ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)],
    &[]
);

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
    ],
    &[attr("a_pos", ID_BACKGROUND_POS_VERTEX_ATTRIBUTE),],
    &[]
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
    ],
    &[attr("a_pos", ID_BACKGROUND_POS_VERTEX_ATTRIBUTE),],
    &[tex("u_image", ID_BACKGROUND_IMAGE_TEXTURE)]
);

// Sky
shader_source!(
    SKY_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<SkyPropsUBO>("SkyPropsUBO", ID_SKY_PROPS_UBO),
    ],
    &[attr("a_pos", ID_SKY_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Atmosphere
shader_source!(
    ATMOSPHERE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<AtmospherePropsUBO>("AtmospherePropsUBO", ID_ATMOSPHERE_PROPS_UBO),
    ],
    &[attr("a_pos", ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Circle
shader_source!(
    CIRCLE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<CircleDrawableUBO, CircleDrawableUBO>("CircleDrawableUBO", ID_CIRCLE_DRAWABLE_UBO),
        block::<CircleEvaluatedPropsUBO>("CircleEvaluatedPropsUBO", ID_CIRCLE_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_CIRCLE_POS_VERTEX_ATTRIBUTE),
        attr("a_color", ID_CIRCLE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_radius", ID_CIRCLE_RADIUS_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_CIRCLE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_CIRCLE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_stroke_color", ID_CIRCLE_STROKE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_stroke_width", ID_CIRCLE_STROKE_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_stroke_opacity", ID_CIRCLE_STROKE_OPACITY_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Collision Box
shader_source!(
    COLLISION_BOX_SHADER,
    &[
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        block::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_COLLISION_POS_VERTEX_ATTRIBUTE),
        attr("a_anchor_pos", ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
        attr("a_extrude", ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
        attr("a_placed", ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
        attr("a_shift", ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
        attr("a_height_offset", ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Collision Circle
shader_source!(
    COLLISION_CIRCLE_SHADER,
    &[
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        block::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_COLLISION_POS_VERTEX_ATTRIBUTE),
        attr("a_anchor_pos", ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
        attr("a_extrude", ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
        attr("a_placed", ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Globe depth
shader_source!(
    GLOBE_DEPTH_SHADER,
    &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),],
    &[attr("a_pos", ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Layer composite
shader_source!(
    LAYER_COMPOSITE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<LayerCompositeUBO>("LayerCompositeUBO", ID_LAYER_COMPOSITE_UBO),
    ],
    &[attr("a_pos", ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)],
    &[tex("u_image", ID_LAYER_COMPOSITE_IMAGE_TEXTURE), tex("u_backdrop", ID_LAYER_COMPOSITE_BACKDROP_TEXTURE)]
);

// Vertex texel
#[cfg(test)]
shader_source!(
    VERTEX_TEXEL_SHADER,
    &[element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)],
    &[attr("a_pos_texel", ID_VERTEX_TEXEL_POS_TEXEL_VERTEX_ATTRIBUTE)],
    &[tex("u_image", ID_VERTEX_TEXEL_TEXTURE)]
);

// Terrain probe
#[cfg(test)]
shader_source!(
    TERRAIN_PROBE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<TerrainPropsUBO>("TerrainPropsUBO", ID_TERRAIN_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_TERRAIN_PROBE_POS_VERTEX_ATTRIBUTE),
        attr("a_dem_coords", ID_TERRAIN_PROBE_DEM_COORDS_VERTEX_ATTRIBUTE)
    ],
    &[tex("u_ground_dem", ID_TERRAIN_DEM_TEXTURE)]
);

// Fill
shader_source!(
    FILL_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<FillDrawableUBO, FillDrawableUnionUBO>("FillDrawableUBO", ID_FILL_DRAWABLE_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Fill Outline
shader_source!(
    FILL_OUTLINE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<FillOutlineDrawableUBO, FillDrawableUnionUBO>("FillOutlineDrawableUBO", ID_FILL_DRAWABLE_UBO),
        block::<FillEvaluatedPropsUBO>("FillEvaluatedPropsUBO", ID_FILL_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE),
        attr("a_outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
    &[]
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
    ],
    &[
        attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_FILL_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_FILL_IMAGE_TEXTURE)]
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
    ],
    &[attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE)],
    &[
        attr("a_outline_pos", ID_FILL_OUTLINE_POS_ATTRIBUTE),
        attr("a_outline_end", ID_FILL_OUTLINE_END_ATTRIBUTE),
        attr("a_outline_color", ID_FILL_OUTLINE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
    ],
    &[]
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
    ],
    &[attr("a_pos", ID_FILL_POS_VERTEX_ATTRIBUTE)],
    &[
        attr("a_outline_pos", ID_FILL_OUTLINE_POS_ATTRIBUTE),
        attr("a_outline_end", ID_FILL_OUTLINE_END_ATTRIBUTE),
        attr("a_color", ID_FILL_COLOR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_FILL_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_FILL_IMAGE_TEXTURE)]
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
shader_source!(
    FILL_EXTRUSION_SHADER,
    FILL_EXTRUSION_BLOCKS,
    &[
        attr("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Fill Extrusion Instanced: the walls, an instance per outline vertex
const FILL_EXTRUSION_INSTANCED_ATTRIBUTES: &[AttributeInfo] = &[attr("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE)];
shader_source!(
    FILL_EXTRUSION_INSTANCED_SHADER,
    FILL_EXTRUSION_BLOCKS,
    FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    &[
        attr("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        attr("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Fill Extrusion Wall: the instanced walls' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer
static FILL_EXTRUSION_WALL_SHADER: ShaderSource = ShaderSource {
    glsl: &glsl_sources::FILL_EXTRUSION_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_BLOCKS,
    attributes: &[
        attr("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        attr("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        attr("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_color", ID_FILL_EXTRUSION_COLOR_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[],
};

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
const FILL_EXTRUSION_PATTERN_TEXTURES: &[TextureInfo] = &[tex("u_image", ID_FILL_EXTRUSION_IMAGE_TEXTURE)];
shader_source!(
    FILL_EXTRUSION_PATTERN_SHADER,
    FILL_EXTRUSION_PATTERN_BLOCKS,
    &[
        attr("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    FILL_EXTRUSION_PATTERN_TEXTURES
);

// Fill Extrusion Pattern Instanced: the walls
shader_source!(
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    FILL_EXTRUSION_PATTERN_BLOCKS,
    FILL_EXTRUSION_INSTANCED_ATTRIBUTES,
    &[
        attr("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        attr("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    FILL_EXTRUSION_PATTERN_TEXTURES
);

// Fill Extrusion Pattern Wall: as Fill Extrusion Wall
static FILL_EXTRUSION_PATTERN_WALL_SHADER: ShaderSource = ShaderSource {
    glsl: &glsl_sources::FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    uniform_blocks: FILL_EXTRUSION_PATTERN_BLOCKS,
    attributes: &[
        attr("a_pos", ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE),
        attr("a_outline_pos", ID_FILL_EXTRUSION_OUTLINE_POS_ATTRIBUTE),
        attr("a_decimals_ed", ID_FILL_EXTRUSION_DECIMALS_ED_ATTRIBUTE),
        attr("a_next_outline_pos", ID_FILL_EXTRUSION_NEXT_OUTLINE_POS_ATTRIBUTE),
        attr("a_next_decimals_ed", ID_FILL_EXTRUSION_NEXT_DECIMALS_ED_ATTRIBUTE),
        attr("a_base", ID_FILL_EXTRUSION_BASE_VERTEX_ATTRIBUTE),
        attr("a_height", ID_FILL_EXTRUSION_HEIGHT_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_FILL_EXTRUSION_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_FILL_EXTRUSION_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: FILL_EXTRUSION_PATTERN_TEXTURES,
};

// Heatmap
shader_source!(
    HEATMAP_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<HeatmapDrawableUBO, HeatmapDrawableUBO>("HeatmapDrawableUBO", ID_HEATMAP_DRAWABLE_UBO),
        block::<HeatmapEvaluatedPropsUBO>("HeatmapEvaluatedPropsUBO", ID_HEATMAP_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_HEATMAP_POS_VERTEX_ATTRIBUTE),
        attr("a_weight", ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE),
        attr("a_radius", ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Heatmap Texture
shader_source!(
    HEATMAP_TEXTURE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        block::<HeatmapTexturePropsUBO>("HeatmapTexturePropsUBO", ID_HEATMAP_TEXTURE_PROPS_UBO),
    ],
    &[attr("a_pos", ID_HEATMAP_POS_VERTEX_ATTRIBUTE),],
    &[tex("u_image", ID_HEATMAP_IMAGE_TEXTURE), tex("u_color_ramp", ID_HEATMAP_COLOR_RAMP_TEXTURE)]
);

// Hillshade Prepare
shader_source!(
    HILLSHADE_PREPARE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<HillshadePrepareDrawableUBO>("HillshadePrepareDrawableUBO", ID_HILLSHADE_PREPARE_DRAWABLE_UBO),
        block::<HillshadePrepareTilePropsUBO>("HillshadePrepareTilePropsUBO", ID_HILLSHADE_PREPARE_TILE_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
        attr("a_texture_pos", ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_HILLSHADE_IMAGE_TEXTURE)]
);

// Hillshade
shader_source!(
    HILLSHADE_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<HillshadeDrawableUBO, HillshadeDrawableUBO>("HillshadeDrawableUBO", ID_HILLSHADE_DRAWABLE_UBO),
        element::<HillshadeTilePropsUBO, HillshadeTilePropsUBO>("HillshadeTilePropsUBO", ID_HILLSHADE_TILE_PROPS_UBO),
        block::<HillshadeEvaluatedPropsUBO>("HillshadeEvaluatedPropsUBO", ID_HILLSHADE_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos", ID_HILLSHADE_POS_VERTEX_ATTRIBUTE),
        attr("a_texture_pos", ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_HILLSHADE_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos", ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE),
        attr("a_texture_pos", ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE),
    ],
    &[
        tex("u_image", ID_COLOR_RELIEF_IMAGE_TEXTURE),
        tex("u_elevation_stops", ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE),
        tex("u_color_stops", ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE)
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
    ],
    &[
        attr("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        attr("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
        attr("a_color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        attr("a_width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Line Gradient
shader_source!(
    LINE_GRADIENT_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        block::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        element::<LineGradientDrawableUBO, LineDrawableUnionUBO>("LineGradientDrawableUBO", ID_LINE_DRAWABLE_UBO),
        block::<LineEvaluatedPropsUBO>("LineEvaluatedPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ],
    &[
        attr("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        attr("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        attr("a_width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_LINE_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        attr("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        attr("a_gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_pattern_from", ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE),
        attr("a_pattern_to", ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_LINE_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        attr("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
        attr("a_color", ID_LINE_COLOR_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        attr("a_width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_LINE_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        attr("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
        attr("a_blur", ID_LINE_BLUR_VERTEX_ATTRIBUTE),
        attr("a_opacity", ID_LINE_OPACITY_VERTEX_ATTRIBUTE),
        attr("a_gapwidth", ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_offset", ID_LINE_OFFSET_VERTEX_ATTRIBUTE),
        attr("a_width", ID_LINE_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_floorwidth", ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE),
        attr("a_dasharray", ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE),
    ],
    &[tex("u_image", ID_LINE_IMAGE_TEXTURE), tex("u_image_dash", ID_LINE_DASH_TEXTURE)]
);

// Raster
shader_source!(
    RASTER_SHADER,
    &[
        element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        element::<RasterDrawableUBO, RasterDrawableUBO>("RasterDrawableUBO", ID_RASTER_DRAWABLE_UBO),
        block::<RasterEvaluatedPropsUBO>("RasterEvaluatedPropsUBO", ID_RASTER_EVALUATED_PROPS_UBO),
    ],
    &[attr("a_pos", ID_RASTER_POS_VERTEX_ATTRIBUTE), attr("a_texture_pos", ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE),],
    &[tex("u_image0", ID_RASTER_IMAGE0_TEXTURE), tex("u_image1", ID_RASTER_IMAGE1_TEXTURE)]
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
    ],
    &[
        attr("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        attr("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        attr("a_pixeloffset", ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
        attr("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        attr("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        attr("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        attr("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        attr("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[tex("u_texture", ID_SYMBOL_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        attr("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        attr("a_pixeloffset", ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
        attr("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        attr("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        attr("a_fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        attr("a_halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        attr("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        attr("a_halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        attr("a_halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
        attr("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        attr("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[tex("u_texture", ID_SYMBOL_IMAGE_TEXTURE)]
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
    ],
    &[
        attr("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        attr("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        attr("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        attr("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        attr("a_fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        attr("a_halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        attr("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        attr("a_halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        attr("a_halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
        attr("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        attr("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[tex("u_texture", ID_SYMBOL_IMAGE_TEXTURE), tex("u_texture_icon", ID_SYMBOL_IMAGE_ICON_TEXTURE)]
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
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
