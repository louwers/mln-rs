//! The programs of the `color-relief` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Stages, Texture};

use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

pub(crate) const DRAWABLE: Buffer = Buffer::storage::<ColorReliefDrawableUBO>("drawableVector", Stages::Vertex);
pub(crate) const TILE_PROPS: Buffer = Buffer::storage::<ColorReliefTilePropsUBO>("tilePropsVector", Stages::Both);
pub(crate) const PROPS: Buffer = Buffer::uniform::<ColorReliefEvaluatedPropsUBO>("props", 1);

// The ids of the programs' textures and vertex attributes.
pub(crate) const ID_COLOR_RELIEF_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: usize = 1;
pub(crate) const ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: usize = 2;
pub(crate) const ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static COLOR_RELIEF: Program = Program {
    name: "color-relief",
    buffers: &[DRAWABLE, TILE_PROPS, PROPS],
    attributes: &[
        Attribute { id: ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, format: Short2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("image"), Texture::unfilterable("elevation_stops"), Texture::new("color_stops")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLOR_RELIEF_SHADER)
        .msl(&msl::COLOR_RELIEF_SHADER)
        .glsl_es(&glsl_es::COLOR_RELIEF_SHADER)
        .glsl(&glsl::COLOR_RELIEF_SHADER),
};
