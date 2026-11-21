//! The programs of the `raster` layer type: the buffers,
//! attributes, textures and paint properties of its shaders, and their
//! sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Sources, Stages, Texture};

use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

// The binding ids of the programs: their buffers, textures and vertex
// attributes.
pub(crate) const ID_RASTER_DRAWABLE_UBO: usize = DRAWABLE.id;
pub(crate) const ID_RASTER_EVALUATED_PROPS_UBO: usize = PROPS.id;
pub(crate) const ID_RASTER_IMAGE0_TEXTURE: usize = 0;
pub(crate) const ID_RASTER_IMAGE1_TEXTURE: usize = 1;
pub(crate) const ID_RASTER_POS_VERTEX_ATTRIBUTE: usize = 0;
pub(crate) const ID_RASTER_TEXTURE_POS_VERTEX_ATTRIBUTE: usize = 1;

const DRAWABLE: Buffer = Buffer::storage::<RasterDrawableUBO>("drawableVector", Stages::Vertex);
const PROPS: Buffer = Buffer::uniform::<RasterEvaluatedPropsUBO>("props", 0);

pub static RASTER: Program = Program {
    name: "raster",
    buffers: &[
        DRAWABLE,
        PROPS,
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
