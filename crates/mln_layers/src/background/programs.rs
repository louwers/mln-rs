//! The programs of the `background` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Stages, Texture};

use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// A layer binds one buffer of `BackgroundDrawableUnionUBO`s for both
// programs, each of which reads its own block of them, and its block of
// properties at the one id.
pub(crate) const DRAWABLE: Buffer =
    Buffer::storage_of::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>("drawableVector", Stages::Vertex);
const PATTERN_DRAWABLE: Buffer =
    Buffer::storage_of::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>("drawableVector", Stages::Vertex);
pub(crate) const PROPS: Buffer = Buffer::uniform::<BackgroundPropsUBO>("props", 0);
const PATTERN_PROPS: Buffer = Buffer::uniform::<BackgroundPatternPropsUBO>("props", 0);
const _: () = assert!(DRAWABLE.id == PATTERN_DRAWABLE.id && PROPS.id == PATTERN_PROPS.id);

// The ids of the programs' textures and vertex attributes.
pub(crate) const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static BACKGROUND: Program = Program {
    name: "background",
    buffers: &[DRAWABLE, PROPS],
    attributes: &[Attribute { id: ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_SHADER)
        .msl(&msl::BACKGROUND_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_SHADER)
        .glsl(&glsl::BACKGROUND_SHADER),
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "background-pattern",
    buffers: &[PATTERN_DRAWABLE, PATTERN_PROPS],
    attributes: &[Attribute { id: ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[Texture::new("image")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_PATTERN_SHADER)
        .msl(&msl::BACKGROUND_PATTERN_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_PATTERN_SHADER)
        .glsl(&glsl::BACKGROUND_PATTERN_SHADER),
};
