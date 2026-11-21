// The tables of the engine's shaders describe the vertex layout the
// renderer produces.

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LINE_DATA_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_SKY_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::source::glsl::{Attribute, Source, Texture};

use crate::front_end::Stage;
use crate::sources;

/// The source of `stage` of `source` for naga's GLSL front end, with
/// `defines` (`#define` lines): the version, the defines, the stage's
/// prelude, and the program's own prelude and stage.
#[must_use]
pub fn compose(source: &Source, stage: Stage, defines: &str) -> String {
    let glsl = match stage {
        Stage::Vertex => source.glsl.vertex,
        Stage::Fragment => source.glsl.fragment,
    };
    crate::front_end::compose(stage, defines, source.glsl.prelude, glsl)
}

use AttributeDataType::{Short2, UByte4};

const CLIPPING_MASK_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)];
static CLIPPING_MASK_PROGRAM: Source = Source {
    glsl: &sources::CLIPPING_MASK_PROGRAM,
    attributes: CLIPPING_MASK_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const GLOBE_DEPTH_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE)];
static GLOBE_DEPTH_SHADER: Source = Source {
    glsl: &sources::GLOBE_DEPTH_SHADER,
    attributes: GLOBE_DEPTH_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const LAYER_COMPOSITE_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE)];
static LAYER_COMPOSITE_SHADER: Source = Source {
    glsl: &sources::LAYER_COMPOSITE_SHADER,
    attributes: LAYER_COMPOSITE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[
        Texture::new(0, ID_LAYER_COMPOSITE_IMAGE_TEXTURE),
        Texture::new(1, ID_LAYER_COMPOSITE_BACKDROP_TEXTURE),
    ],
};

const TILE_DEBUG_ATTRIBUTES: &[Attribute] = &[
    Attribute::new(0, Short2, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE),
    Attribute::new(1, UByte4, ID_LINE_DATA_VERTEX_ATTRIBUTE),
];

static TILE_DEBUG_SHADER: Source = Source {
    glsl: &sources::TILE_DEBUG_SHADER,
    attributes: TILE_DEBUG_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

const SKY_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_SKY_POS_VERTEX_ATTRIBUTE)];
const ATMOSPHERE_ATTRIBUTES: &[Attribute] = &[Attribute::new(0, Short2, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE)];
static SKY_SHADER: Source =
    Source { glsl: &sources::SKY_SHADER, attributes: SKY_ATTRIBUTES, instance_attributes: &[], textures: &[] };
static ATMOSPHERE_SHADER: Source = Source {
    glsl: &sources::ATMOSPHERE_SHADER,
    attributes: ATMOSPHERE_ATTRIBUTES,
    instance_attributes: &[],
    textures: &[],
};

/// The source of each of the engine's own programs.
pub static BUILT_INS: &[(&Program, &Source)] = &[
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TILE_DEBUG, &TILE_DEBUG_SHADER),
];
