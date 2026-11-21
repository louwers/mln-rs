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

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_BACKGROUND_DRAWABLE_UBO: usize = Stages::Vertex.id();
pub(crate) const ID_BACKGROUND_PROPS_UBO: usize = PROPS.id;

const PROPS: Buffer = Buffer::uniform::<BackgroundPropsUBO>("props", 0);
pub(crate) const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static BACKGROUND: Program = Program {
    name: "background",
    buffers: &[
        Buffer::storage_of::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>("drawableVector", Stages::Vertex),
        PROPS,
    ],
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
    buffers: &[
        Buffer::storage_of::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(
            "drawableVector",
            Stages::Vertex,
        ),
        Buffer::uniform::<BackgroundPatternPropsUBO>("props", 0),
    ],
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
