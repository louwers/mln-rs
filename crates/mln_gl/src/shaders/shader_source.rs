// The GLSL of the engine's shaders, and the tables of where their uniform
// blocks, attributes and textures bind:
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
    ID_GLOBAL_PAINT_PARAMS_UBO, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE,
    ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_UBO,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SKY_PROPS_UBO,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::shaders::source::glsl_es::{Attribute, Source, Texture, UniformBlock};
use mln_gfx::shaders::tile_debug_ubo::{TileDebugDrawableElement, TileDebugDrawableUBO, TileDebugPropsUBO};

use super::glsl_sources;

// Clipping mask
static CLIPPING_MASK_PROGRAM: Source = Source {
    glsl: &glsl_sources::CLIPPING_MASK_PROGRAM,
    uniform_blocks: &[UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)],
    attributes: &[Attribute::new("a_pos", ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[],
};

// Sky
static SKY_SHADER: Source = Source {
    glsl: &glsl_sources::SKY_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<SkyPropsUBO>("SkyPropsUBO", ID_SKY_PROPS_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_SKY_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[],
};

// Atmosphere
static ATMOSPHERE_SHADER: Source = Source {
    glsl: &glsl_sources::ATMOSPHERE_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<AtmospherePropsUBO>("AtmospherePropsUBO", ID_ATMOSPHERE_PROPS_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[],
};

// Globe depth
static GLOBE_DEPTH_SHADER: Source = Source {
    glsl: &glsl_sources::GLOBE_DEPTH_SHADER,
    uniform_blocks: &[UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO)],
    attributes: &[Attribute::new("a_pos", ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[],
};

// Layer composite
static LAYER_COMPOSITE_SHADER: Source = Source {
    glsl: &glsl_sources::LAYER_COMPOSITE_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<LayerCompositeUBO>("LayerCompositeUBO", ID_LAYER_COMPOSITE_UBO),
    ],
    attributes: &[Attribute::new("a_pos", ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)],
    instance_attributes: &[],
    textures: &[
        Texture::new("u_image", ID_LAYER_COMPOSITE_IMAGE_TEXTURE),
        Texture::new("u_backdrop", ID_LAYER_COMPOSITE_BACKDROP_TEXTURE),
    ],
};

// Tile debug
static TILE_DEBUG_SHADER: Source = Source {
    glsl: &glsl_sources::TILE_DEBUG_SHADER,
    uniform_blocks: &[
        UniformBlock::element::<ProjectionUBO, ProjectionUBO>("ProjectionUBO", ID_PROJECTION_UBO),
        UniformBlock::new::<GlobalPaintParamsUBO>("GlobalPaintParamsUBO", ID_GLOBAL_PAINT_PARAMS_UBO),
        UniformBlock::element::<TileDebugDrawableUBO, TileDebugDrawableElement>(
            "TileDebugDrawableUBO",
            ID_LINE_DRAWABLE_UBO,
        ),
        UniformBlock::new::<TileDebugPropsUBO>("TileDebugPropsUBO", ID_LINE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute::new("a_pos_normal", ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
        Attribute::new("a_data", ID_LINE_DATA_VERTEX_ATTRIBUTE),
    ],
    instance_attributes: &[],
    textures: &[],
};

/// The source of each of the engine's own programs.
pub(crate) static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
];
