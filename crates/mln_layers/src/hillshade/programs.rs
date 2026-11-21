//! The programs of the `hillshade` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::bindings::{FIRST_UNIFORM_ID, FRAGMENT_STORAGE_ID, VERTEX_STORAGE_ID};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Texture};

use super::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_HILLSHADE_DRAWABLE_UBO: usize = VERTEX_STORAGE_ID;
pub(crate) const ID_HILLSHADE_TILE_PROPS_UBO: usize = FRAGMENT_STORAGE_ID;
pub(crate) const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = FIRST_UNIFORM_ID;
pub(crate) const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = VERTEX_STORAGE_ID;
pub(crate) const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = FIRST_UNIFORM_ID;
pub(crate) const ID_HILLSHADE_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_HILLSHADE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static HILLSHADE: Program = Program {
    name: "hillshade",
    buffers: &[
        Buffer::storage::<HillshadeDrawableUBO>(ID_HILLSHADE_DRAWABLE_UBO),
        Buffer::storage::<HillshadeTilePropsUBO>(ID_HILLSHADE_TILE_PROPS_UBO),
        Buffer::uniform::<HillshadeEvaluatedPropsUBO>(ID_HILLSHADE_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, format: Short2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE_SHADER)
        .msl(&msl::HILLSHADE_SHADER)
        .glsl_es(&glsl_es::HILLSHADE_SHADER)
        .glsl(&glsl::HILLSHADE_SHADER),
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "hillshade-prepare",
    buffers: &[
        Buffer::uniform::<HillshadePrepareDrawableUBO>(ID_HILLSHADE_PREPARE_DRAWABLE_UBO),
        Buffer::uniform::<HillshadePrepareTilePropsUBO>(ID_HILLSHADE_PREPARE_TILE_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, format: Short2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::HILLSHADE_PREPARE_SHADER)
        .msl(&msl::HILLSHADE_PREPARE_SHADER)
        .glsl_es(&glsl_es::HILLSHADE_PREPARE_SHADER)
        .glsl(&glsl::HILLSHADE_PREPARE_SHADER),
};
