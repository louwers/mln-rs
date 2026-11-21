//! The programs of the `raster` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Texture};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_RASTER_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO;
pub(crate) const ID_RASTER_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub(crate) const ID_RASTER_IMAGE0_TEXTURE: usize = 0;
pub(crate) const ID_RASTER_IMAGE1_TEXTURE: usize = 1;
pub(crate) const ID_RASTER_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static RASTER: Program = Program {
    name: "raster",
    buffers: &[
        Buffer::storage::<RasterDrawableUBO>(ID_RASTER_DRAWABLE_UBO),
        Buffer::uniform::<RasterEvaluatedPropsUBO>(ID_RASTER_EVALUATED_PROPS_UBO),
    ],
    attributes: &[
        Attribute { id: ID_RASTER_POS_VERTEX_ATTRIBUTE, format: Short2 },
        Attribute { id: ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, format: Short2 },
    ],
    instance_attributes: &[],
    textures: &[Texture::new("u_image0"), Texture::new("u_image1")],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::RASTER_SHADER)
        .msl(&msl::RASTER_SHADER)
        .glsl_es(&glsl_es::RASTER_SHADER)
        .glsl(&glsl::RASTER_SHADER),
};
