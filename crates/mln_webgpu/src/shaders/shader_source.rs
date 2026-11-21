use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::programs::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_SKY_POS_VERTEX_ATTRIBUTE, ID_TILE_DEBUG_DATA_VERTEX_ATTRIBUTE, ID_TILE_DEBUG_POS_NORMAL_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::source::wgsl::{Attribute, Source, Texture};

use super::wgsl_sources;

const CLIPPING_MASK_ATTRIBUTES: &[Attribute] = &[Attribute { index: 2, id: ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE }];

static CLIPPING_MASK_PROGRAM: Source = Source {
    wgsl: &wgsl_sources::CLIPPING_MASK_PROGRAM,
    attributes: CLIPPING_MASK_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const SKY_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, ID_SKY_POS_VERTEX_ATTRIBUTE)];
static SKY_SHADER: Source =
    Source { wgsl: &wgsl_sources::SKY_SHADER, attributes: SKY_ATTRIBUTES, instance_attributes: &[], textures: &[] };
const ATMOSPHERE_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
static ATMOSPHERE_SHADER: Source = Source {
    wgsl: &wgsl_sources::ATMOSPHERE_SHADER,
    attributes: ATMOSPHERE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const GLOBE_DEPTH_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
static GLOBE_DEPTH_SHADER: Source = Source {
    wgsl: &wgsl_sources::GLOBE_DEPTH_SHADER,
    attributes: GLOBE_DEPTH_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const TILE_DEBUG_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(4, ID_TILE_DEBUG_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(5, ID_TILE_DEBUG_DATA_VERTEX_ATTRIBUTE),
];
static TILE_DEBUG_SHADER: Source = Source {
    wgsl: &wgsl_sources::TILE_DEBUG_SHADER,
    attributes: TILE_DEBUG_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const LAYER_COMPOSITE_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)];
const LAYER_COMPOSITE_TEXTURES: &[Texture] =
    &[Texture::new(0, ID_LAYER_COMPOSITE_IMAGE_TEXTURE), Texture::new(1, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE)];
static LAYER_COMPOSITE_SHADER: Source = Source {
    wgsl: &wgsl_sources::LAYER_COMPOSITE_SHADER,
    attributes: LAYER_COMPOSITE_ATTRIBUTES,
    instance_attributes: &[],
    textures: LAYER_COMPOSITE_TEXTURES,
};

/// The source of each of the engine's own programs.
pub(crate) static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
];

#[must_use]
pub(crate) fn prelude() -> &'static str {
    wgsl_sources::PRELUDE.prelude.expect("Prelude without prelude")
}
