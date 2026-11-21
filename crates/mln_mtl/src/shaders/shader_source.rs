// The tables of the engine's shaders describe the vertex layout the
// renderer produces: the vertex buffers follow the shader's UBOs, from
// `<layer>UBOCount`.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::programs::{
    ATMOSPHERE_LAYER_UBO_COUNT, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE,
    ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE,
    ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE,
    ID_TILE_DEBUG_END_ATTRIBUTE, ID_TILE_DEBUG_START_ATTRIBUTE, LAYER_COMPOSITE_UBO_COUNT, SKY_LAYER_UBO_COUNT,
    TILE_DEBUG_UBO_COUNT,
};
use mln_gfx::shaders::shader_defines::DRAWABLE_RESERVED_UBO_COUNT;
use mln_gfx::shaders::source::msl::{Attribute, Source, Texture};

use super::msl_sources;

// The number of UBOs each shader binds before its vertex buffers, for the
// programs whose count `programs` doesn't name.
const ATMOSPHERE_UBO_COUNT: usize = ATMOSPHERE_LAYER_UBO_COUNT;
const CLIPPING_MASK_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const GLOBE_DEPTH_UBO_COUNT: usize = DRAWABLE_RESERVED_UBO_COUNT;
const SKY_UBO_COUNT: usize = SKY_LAYER_UBO_COUNT;

use AttributeDataType::Short2;

const CLIPPING_MASK_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, CLIPPING_MASK_UBO_COUNT, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
static CLIPPING_MASK_PROGRAM: Source = Source {
    msl: &msl_sources::CLIPPING_MASK_PROGRAM,
    attributes: CLIPPING_MASK_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const GLOBE_DEPTH_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, GLOBE_DEPTH_UBO_COUNT, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
static GLOBE_DEPTH_SHADER: Source = Source {
    msl: &msl_sources::GLOBE_DEPTH_SHADER,
    attributes: GLOBE_DEPTH_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const LAYER_COMPOSITE_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, LAYER_COMPOSITE_UBO_COUNT, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)];
static LAYER_COMPOSITE_SHADER: Source = Source {
    msl: &msl_sources::LAYER_COMPOSITE_SHADER,
    attributes: LAYER_COMPOSITE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[
        Texture::new(0, ID_LAYER_COMPOSITE_IMAGE_TEXTURE),
        Texture::new(1, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE),
    ],
};

const TILE_DEBUG_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, TILE_DEBUG_UBO_COUNT, ID_TILE_DEBUG_CORNER_VERTEX_ATTRIBUTE)];
// The start and end of a segment, from one buffer.
const TILE_DEBUG_INSTANCE_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(1, Short2, TILE_DEBUG_UBO_COUNT + 1, ID_TILE_DEBUG_START_ATTRIBUTE),
    Attribute::new(2, Short2, TILE_DEBUG_UBO_COUNT + 1, ID_TILE_DEBUG_END_ATTRIBUTE),
];

static TILE_DEBUG_SHADER: Source = Source {
    msl: &msl_sources::TILE_DEBUG_SHADER,
    attributes: TILE_DEBUG_ATTRIBUTES,
    instance_attributes: TILE_DEBUG_INSTANCE_ATTRIBUTES,
    textures: &[],
};

const SKY_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, SKY_UBO_COUNT, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[Attribute] =
    &[Attribute::new(0, Short2, ATMOSPHERE_UBO_COUNT, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
static SKY_SHADER: Source =
    Source { msl: &msl_sources::SKY_SHADER, attributes: SKY_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static ATMOSPHERE_SHADER: Source = Source {
    msl: &msl_sources::ATMOSPHERE_SHADER,
    attributes: ATMOSPHERE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

/// The source of each of the engine's own programs.
pub(crate) static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
];
