//! The programs of the `hillshade` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Texture};
use mln_gfx::shaders::shader_defines::{
    DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO,
};

use super::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO,
};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_HILLSHADE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_HILLSHADE_TILE_PROPS_UBO: usize = ID_DRAWABLE_RESERVED_FRAGMENT_ONLY_UBO;
pub(crate) const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_HILLSHADE_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_HILLSHADE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static HILLSHADE: Program = Program {
    name: "HillshadeShader",
    buffers: &[
        Buffer::storage::<HillshadeDrawableUBO>(ID_HILLSHADE_DRAWABLE_UBO, "HillshadeDrawableUBO"),
        Buffer::storage::<HillshadeTilePropsUBO>(ID_HILLSHADE_TILE_PROPS_UBO, "HillshadeTilePropsUBO"),
        Buffer::uniform::<HillshadeEvaluatedPropsUBO>(ID_HILLSHADE_EVALUATED_PROPS_UBO, "HillshadeEvaluatedPropsUBO"),
    ],
    attributes: &[
        Attribute::new(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, "a_texture_pos", Short2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: sources!(
        wgsl: HILLSHADE_SHADER,
        msl: HILLSHADE_SHADER,
        glsl_es: HILLSHADE_SHADER,
        glsl: HILLSHADE_SHADER,
    ),
};

pub static HILLSHADE_PREPARE: Program = Program {
    name: "HillshadePrepareShader",
    buffers: &[
        Buffer::uniform::<HillshadePrepareDrawableUBO>(
            ID_HILLSHADE_PREPARE_DRAWABLE_UBO,
            "HillshadePrepareDrawableUBO",
        ),
        Buffer::uniform::<HillshadePrepareTilePropsUBO>(
            ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
            "HillshadePrepareTilePropsUBO",
        ),
    ],
    attributes: &[
        Attribute::new(ID_HILLSHADE_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE, "a_texture_pos", Short2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image")],
    properties: &[],
    sources: sources!(
        wgsl: HILLSHADE_PREPARE_SHADER,
        msl: HILLSHADE_PREPARE_SHADER,
        glsl_es: HILLSHADE_PREPARE_SHADER,
        glsl: HILLSHADE_PREPARE_SHADER,
    ),
};
