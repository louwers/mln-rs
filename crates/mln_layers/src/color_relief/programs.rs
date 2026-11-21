//! The programs of the `color-relief` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Texture};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_COLOR_RELIEF_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_COLOR_RELIEF_TILE_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_COLOR_RELIEF_EVALUATED_PROPS_UBO: usize = ID_COLOR_RELIEF_TILE_PROPS_UBO + 1;
pub(crate) const ID_COLOR_RELIEF_IMAGE_TEXTURE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE: usize = 1;
pub(crate) const ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE: usize = 2;
pub(crate) const ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static COLOR_RELIEF: Program = Program {
    name: "ColorReliefShader",
    buffers: &[
        Buffer::storage::<ColorReliefDrawableUBO>(ID_COLOR_RELIEF_DRAWABLE_UBO, "ColorReliefDrawableUBO"),
        Buffer::storage::<ColorReliefTilePropsUBO>(ID_COLOR_RELIEF_TILE_PROPS_UBO, "ColorReliefTilePropsUBO"),
        Buffer::uniform::<ColorReliefEvaluatedPropsUBO>(
            ID_COLOR_RELIEF_EVALUATED_PROPS_UBO,
            "ColorReliefEvaluatedPropsUBO",
        ),
    ],
    attributes: &[
        Attribute::new(ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, "a_texture_pos", Short2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image"), Texture::unfilterable("u_elevation_stops"), Texture::new("u_color_stops")],
    properties: &[],
    sources: sources!(
        wgsl: COLOR_RELIEF_SHADER,
        msl: COLOR_RELIEF_SHADER,
        glsl_es: COLOR_RELIEF_SHADER,
        glsl: COLOR_RELIEF_SHADER,
    ),
};
