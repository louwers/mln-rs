//! The programs of the `hillshade` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Stages, Texture};

use super::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO,
};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_HILLSHADE_DRAWABLE_UBO: usize = DRAWABLE.id;
pub(crate) const ID_HILLSHADE_TILE_PROPS_UBO: usize = TILE_PROPS.id;
pub(crate) const ID_HILLSHADE_EVALUATED_PROPS_UBO: usize = PROPS.id;
pub(crate) const ID_HILLSHADE_PREPARE_DRAWABLE_UBO: usize = PREPARE_DRAWABLE.id;
pub(crate) const ID_HILLSHADE_PREPARE_TILE_PROPS_UBO: usize = PREPARE_TILE_PROPS.id;
pub(crate) const ID_HILLSHADE_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_HILLSHADE_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_HILLSHADE_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

const DRAWABLE: Buffer = Buffer::storage::<HillshadeDrawableUBO>("drawableVector", Stages::Vertex);
const TILE_PROPS: Buffer = Buffer::storage::<HillshadeTilePropsUBO>("tilePropsVector", Stages::Fragment);
const PROPS: Buffer = Buffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 0);
/// A DEM tile's values, which its prepare draw binds alone.
const PREPARE_DRAWABLE: Buffer = Buffer::draw_uniform::<HillshadePrepareDrawableUBO>("drawable", Stages::Vertex);
const PREPARE_TILE_PROPS: Buffer = Buffer::draw_uniform::<HillshadePrepareTilePropsUBO>("tileProps", Stages::Both);

pub static HILLSHADE: Program = Program {
    name: "hillshade",
    buffers: &[
        DRAWABLE,
        TILE_PROPS,
        PROPS,
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
        PREPARE_DRAWABLE,
        PREPARE_TILE_PROPS,
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
