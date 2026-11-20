// The GLSL of the engine's shaders and the symbol layer type's, and the
// tables of where its uniform blocks, attributes and textures bind:
// - The ids are shader_defines.rs's, so a shader's uniform blocks bind where
//   the renderer puts their buffers.
// - A block's entry has the size of its UBO, which the compile test checks
//   against the program, and, where the renderer writes the block for every
//   drawable of a layer into one buffer, the size of that buffer's elements.
// - Every block a shader's GLSL declares has an entry, the prelude's
//   projection block too: WebGL2 rejects a draw whose program has a block
//   without a buffer, whether or not the shader reads it.

use mln_gfx::shaders::layer_composite_ubo::LayerCompositeUBO;
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_ATMOSPHERE_PROPS_UBO, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE,
    ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE, ID_COLLISION_DRAWABLE_UBO, ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE,
    ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE, ID_COLLISION_PLACED_VERTEX_ATTRIBUTE,
    ID_COLLISION_POS_VERTEX_ATTRIBUTE, ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE, ID_COLLISION_TILE_PROPS_UBO,
    ID_GLOBAL_PAINT_PARAMS_UBO, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE,
    ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_UBO,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SKY_PROPS_UBO,
    ID_SYMBOL_COLOR_ATTRIBUTE, ID_SYMBOL_DATA_ATTRIBUTE, ID_SYMBOL_DRAWABLE_UBO, ID_SYMBOL_EVALUATED_PROPS_UBO,
    ID_SYMBOL_FADE_OPACITY_ATTRIBUTE, ID_SYMBOL_HALO_BLUR_ATTRIBUTE, ID_SYMBOL_HALO_COLOR_ATTRIBUTE,
    ID_SYMBOL_HALO_WIDTH_ATTRIBUTE, ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE, ID_SYMBOL_IMAGE_ICON_TEXTURE,
    ID_SYMBOL_IMAGE_TEXTURE, ID_SYMBOL_OPACITY_ATTRIBUTE, ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE,
    ID_SYMBOL_POS_OFFSET_ATTRIBUTE, ID_SYMBOL_PROJECTED_POS_ATTRIBUTE, ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE,
    ID_SYMBOL_TILE_PROPS_UBO,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::shaders::source::glsl_es::{Attribute, Source, Texture, UniformBlock};
use mln_gfx::shaders::symbol_layer_ubo::{
    CollisionDrawableUBO, CollisionTilePropsUBO, SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO,
};
use mln_gfx::shaders::tile_debug_ubo::{TileDebugDrawableElement, TileDebugDrawableUBO, TileDebugPropsUBO};

use super::glsl_sources;

macro_rules! shader_source {
    ($name:ident, $blocks:expr, $attrs:expr, $textures:expr) => {
        shader_source!($name, $blocks, $attrs, &[], $textures);
    };
    ($name:ident, $blocks:expr, $attrs:expr, $instance_attrs:expr, $textures:expr) => {
        static $name: Source = Source {
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
    &[UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)],
    &[Attribute::new("a_pos", ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)],
    &[]
);

// Sky
shader_source!(
    SKY_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<SkyPropsUBO>("SkyPropsUBO", ID_SKY_PROPS_UBO),
    ],
    &[Attribute::new("a_pos", ID_SKY_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Atmosphere
shader_source!(
    ATMOSPHERE_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<AtmospherePropsUBO>("AtmospherePropsUBO", ID_ATMOSPHERE_PROPS_UBO),
    ],
    &[Attribute::new("a_pos", ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Collision Box
shader_source!(
    COLLISION_BOX_SHADER,
    &[
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        UniformBlock::new::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos", ID_COLLISION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_anchor_pos", ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_extrude", ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
        Attribute::new("a_placed", ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
        Attribute::new("a_shift", ID_COLLISION_SHIFT_VERTEX_ATTRIBUTE),
        Attribute::new("a_height_offset", ID_COLLISION_HEIGHT_OFFSET_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Collision Circle
shader_source!(
    COLLISION_CIRCLE_SHADER,
    &[
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<CollisionDrawableUBO>("CollisionDrawableUBO", ID_COLLISION_DRAWABLE_UBO),
        UniformBlock::new::<CollisionTilePropsUBO>("CollisionTilePropsUBO", ID_COLLISION_TILE_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos", ID_COLLISION_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_anchor_pos", ID_COLLISION_ANCHOR_POS_VERTEX_ATTRIBUTE),
        Attribute::new("a_extrude", ID_COLLISION_EXTRUDE_VERTEX_ATTRIBUTE),
        Attribute::new("a_placed", ID_COLLISION_PLACED_VERTEX_ATTRIBUTE),
    ],
    &[]
);

// Globe depth
shader_source!(
    GLOBE_DEPTH_SHADER,
    &[UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),],
    &[Attribute::new("a_pos", ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE),],
    &[]
);

// Layer composite
shader_source!(
    LAYER_COMPOSITE_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<LayerCompositeUBO>("LayerCompositeUBO", ID_LAYER_COMPOSITE_UBO),
    ],
    &[Attribute::new("a_pos", ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)],
    &[
        Texture::new("u_image", ID_LAYER_COMPOSITE_IMAGE_TEXTURE),
        Texture::new("u_backdrop", ID_LAYER_COMPOSITE_BACKDROP_TEXTURE)
    ]
);

// Tile debug
shader_source!(
    TILE_DEBUG_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<TileDebugDrawableUBO, TileDebugDrawableElement>(
            "TileDebugDrawableUBO",
            ID_LINE_DRAWABLE_UBO
        ),
        UniformBlock::new::<TileDebugPropsUBO>("TileDebugPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        Attribute::new("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE)
    ],
    &[]
);

// Symbol Icon
shader_source!(
    SYMBOL_ICON_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        UniformBlock::element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        UniformBlock::new::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        Attribute::new("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        Attribute::new("a_pixeloffset", ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
        Attribute::new("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        Attribute::new("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        Attribute::new("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Attribute::new("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        Attribute::new("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[Texture::new("u_texture", ID_SYMBOL_IMAGE_TEXTURE)]
);

// Symbol SDF
shader_source!(
    SYMBOL_SDF_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        UniformBlock::element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        UniformBlock::new::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        Attribute::new("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        Attribute::new("a_pixeloffset", ID_SYMBOL_PIXEL_OFFSET_ATTRIBUTE),
        Attribute::new("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        Attribute::new("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        Attribute::new("a_fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Attribute::new("a_halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Attribute::new("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Attribute::new("a_halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Attribute::new("a_halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
        Attribute::new("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        Attribute::new("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[Texture::new("u_texture", ID_SYMBOL_IMAGE_TEXTURE)]
);

// Symbol Text & Icon
shader_source!(
    SYMBOL_TEXT_AND_ICON_SHADER,
    &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<SymbolDrawableUBO, SymbolDrawableUBO>("SymbolDrawableUBO", ID_SYMBOL_DRAWABLE_UBO),
        UniformBlock::element::<SymbolTilePropsUBO, SymbolTilePropsUBO>("SymbolTilePropsUBO", ID_SYMBOL_TILE_PROPS_UBO),
        UniformBlock::new::<SymbolEvaluatedPropsUBO>("SymbolEvaluatedPropsUBO", ID_SYMBOL_EVALUATED_PROPS_UBO),
    ],
    &[
        Attribute::new("a_pos_offset", ID_SYMBOL_POS_OFFSET_ATTRIBUTE),
        Attribute::new("a_data", ID_SYMBOL_DATA_ATTRIBUTE),
        Attribute::new("a_projected_pos", ID_SYMBOL_PROJECTED_POS_ATTRIBUTE),
        Attribute::new("a_fade_opacity", ID_SYMBOL_FADE_OPACITY_ATTRIBUTE),
        Attribute::new("a_fill_color", ID_SYMBOL_COLOR_ATTRIBUTE),
        Attribute::new("a_halo_color", ID_SYMBOL_HALO_COLOR_ATTRIBUTE),
        Attribute::new("a_opacity", ID_SYMBOL_OPACITY_ATTRIBUTE),
        Attribute::new("a_halo_width", ID_SYMBOL_HALO_WIDTH_ATTRIBUTE),
        Attribute::new("a_halo_blur", ID_SYMBOL_HALO_BLUR_ATTRIBUTE),
        Attribute::new("a_tex_origin", ID_SYMBOL_TEX_ORIGIN_ATTRIBUTE),
        Attribute::new("a_height_offset", ID_SYMBOL_HEIGHT_OFFSET_ATTRIBUTE),
    ],
    &[Texture::new("u_texture", ID_SYMBOL_IMAGE_TEXTURE), Texture::new("u_texture_icon", ID_SYMBOL_IMAGE_ICON_TEXTURE)]
);

/// The source of each of the engine's own programs, and of the symbol layer
/// type's, which is in the engine.
pub(crate) static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
];
