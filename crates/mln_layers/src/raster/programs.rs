//! The programs of the `raster` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Texture};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use crate::shaders::sources;

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_RASTER_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_RASTER_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_RASTER_IMAGE0_TEXTURE: usize = 0;
pub(crate) const ID_RASTER_IMAGE1_TEXTURE: usize = 1;
pub(crate) const ID_RASTER_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static RASTER: Program = Program {
    name: "RasterShader",
    buffers: &[
        Buffer::storage::<RasterDrawableUBO>(ID_RASTER_DRAWABLE_UBO, "RasterDrawableUBO"),
        Buffer::uniform::<RasterEvaluatedPropsUBO>(ID_RASTER_EVALUATED_PROPS_UBO, "RasterEvaluatedPropsUBO"),
    ],
    attributes: &[
        Attribute::new(ID_RASTER_POS_VERTEX_ATTRIBUTE, "a_pos", Short2),
        Attribute::new(ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, "a_texture_pos", Short2),
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image0"), Texture::new("u_image1")],
    properties: &[],
    sources: sources!(wgsl: RASTER_SHADER, msl: RASTER_SHADER, glsl_es: RASTER_SHADER, glsl: RASTER_SHADER,),
};
