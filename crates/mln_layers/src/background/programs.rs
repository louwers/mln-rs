//! The programs of the `background` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::bindings::{FIRST_UNIFORM_ID, VERTEX_STORAGE_ID};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Texture};

use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_BACKGROUND_DRAWABLE_UBO: usize = VERTEX_STORAGE_ID;
pub(crate) const ID_BACKGROUND_PROPS_UBO: usize = FIRST_UNIFORM_ID;
pub(crate) const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static BACKGROUND: Program = Program {
    name: "background",
    buffers: &[
        Buffer::storage_of::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>(ID_BACKGROUND_DRAWABLE_UBO),
        Buffer::uniform::<BackgroundPropsUBO>(ID_BACKGROUND_PROPS_UBO),
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
        Buffer::storage_of::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(ID_BACKGROUND_DRAWABLE_UBO),
        Buffer::uniform::<BackgroundPatternPropsUBO>(ID_BACKGROUND_PROPS_UBO),
    ],
    attributes: &[Attribute { id: ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, format: Short2 }],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::BACKGROUND_PATTERN_SHADER)
        .msl(&msl::BACKGROUND_PATTERN_SHADER)
        .glsl_es(&glsl_es::BACKGROUND_PATTERN_SHADER)
        .glsl(&glsl::BACKGROUND_PATTERN_SHADER),
};
