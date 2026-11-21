//! The programs of the `raster` layer type: the attributes, textures and
//! paint properties of its shaders.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub const ID_RASTER_DRAWABLE_UBO: usize = ID_DRAWABLE_RESERVED_VERTEX_ONLY_UBO; // SSBO
pub const ID_RASTER_EVALUATED_PROPS_UBO: usize = DRAWABLE_RESERVED_UBO_COUNT;
pub const RASTER_LAYER_UBO_COUNT: usize = ID_RASTER_EVALUATED_PROPS_UBO + 1;
pub const ID_RASTER_IMAGE0_TEXTURE: usize = 0;
pub const ID_RASTER_IMAGE1_TEXTURE: usize = 1;
pub const ID_RASTER_POS_VERTEX_ATTRIBUTE: usize = 0;
pub const ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

pub static RASTER: Program = Program {
    name: "RasterShader",
    attributes: &[(ID_RASTER_POS_VERTEX_ATTRIBUTE, Short2), (ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE, Short2)],
    instance_attributes: &[],
    textures: &[ID_RASTER_IMAGE0_TEXTURE, ID_RASTER_IMAGE1_TEXTURE],
    properties: &[],
};
