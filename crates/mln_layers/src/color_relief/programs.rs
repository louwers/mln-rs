//! The programs of the `color-relief` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::bindings::{FIRST_UNIFORM_ID, VERTEX_STORAGE_ID};
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Texture};

use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_COLOR_RELIEF_DRAWABLE_UBO: usize = VERTEX_STORAGE_ID;
pub(crate) const ID_COLOR_RELIEF_TILE_PROPS_UBO: usize = FIRST_UNIFORM_ID;
pub(crate) const ID_COLOR_RELIEF_EVALUATED_PROPS_UBO: usize = ID_COLOR_RELIEF_TILE_PROPS_UBO + 1;
pub(crate) const ID_COLOR_RELIEF_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: usize = 1;
pub(crate) const ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: usize = 2;
pub(crate) const ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static COLOR_RELIEF: Program = Program {
    name: "color-relief",
    buffers: &[
        Buffer::storage::<ColorReliefDrawableUBO>(ID_COLOR_RELIEF_DRAWABLE_UBO),
        Buffer::storage::<ColorReliefTilePropsUBO>(ID_COLOR_RELIEF_TILE_PROPS_UBO),
        Buffer::uniform::<ColorReliefEvaluatedPropsUBO>(ID_COLOR_RELIEF_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, format: Short2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image"), Texture::unfilterable("u_elevation_stops"), Texture::new("u_color_stops")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::COLOR_RELIEF_SHADER)
        .msl(&msl::COLOR_RELIEF_SHADER)
        .glsl_es(&glsl_es::COLOR_RELIEF_SHADER)
        .glsl(&glsl::COLOR_RELIEF_SHADER),
};
