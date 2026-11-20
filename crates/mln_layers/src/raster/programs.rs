//! The programs of the `raster` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{
    ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE, ID_RASTER_POS_VERTEX_ATTRIBUTE,
    ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE,
};

pub static RASTER: Program = Program {
    name: "RasterShader",
    attributes: &[(ID_RASTER_POS_VERTEX_ATTRIBUTE, Short2), (ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE],
    properties: &[],
};
