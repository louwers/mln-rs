//! The programs of the `background` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Texture};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_BACKGROUND_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_BACKGROUND_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_BACKGROUND_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_BACKGROUND_POS_VERTEX_ATTRIBUTE: usize = 0;

pub static BACKGROUND: Program = Program {
    name: "BackgroundShader",
    buffers: &[
        Buffer::storage_of::<BackgroundDrawableUBO, BackgroundDrawableUnionUBO>(ID_BACKGROUND_DRAWABLE_UBO),
        Buffer::uniform::<BackgroundPropsUBO>(ID_BACKGROUND_PROPS_UBO),
    ],
    attributes: &[Attribute::new(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, "a_pos", Short2)],
    instance_attributes: &[],
    textures: &[],
    properties: &[],
    sources: sources!(
        wgsl: BACKGROUND_SHADER,
        msl: BACKGROUND_SHADER,
        glsl_es: BACKGROUND_SHADER,
        glsl: BACKGROUND_SHADER,
    ),
};

pub static BACKGROUND_PATTERN: Program = Program {
    name: "BackgroundPatternShader",
    buffers: &[
        Buffer::storage_of::<BackgroundPatternDrawableUBO, BackgroundDrawableUnionUBO>(ID_BACKGROUND_DRAWABLE_UBO),
        Buffer::uniform::<BackgroundPatternPropsUBO>(ID_BACKGROUND_PROPS_UBO),
    ],
    attributes: &[Attribute::new(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, "a_pos", Short2)],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: sources!(
        wgsl: BACKGROUND_PATTERN_SHADER,
        msl: BACKGROUND_PATTERN_SHADER,
        glsl_es: BACKGROUND_PATTERN_SHADER,
        glsl: BACKGROUND_PATTERN_SHADER,
    ),
};
