//! The programs of the `raster` layer type: the buffers, attributes and
//! textures of its shader, and its sources.

use mln_gfx::gfx_types::AttributeDataType::Short2;
use mln_gfx::shaders::program::{Attribute, Buffer, Program, Projection, Sources, Stages, Texture};

use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};

/// A tile's image, or an image source's.
pub static RASTER: Program = Program {
    name: "raster",
    buffers: &[
        Buffer::storage::<RasterDrawableUBO>("drawableVector", Stages::Vertex),
        Buffer::uniform::<RasterEvaluatedPropsUBO>("props", 0),
    ],
    attributes: &[Attribute::new("pos", Short2), Attribute::new("texture_pos", Short2)],
    instance_attributes: &[],
    textures: &[Texture::new("image0"), Texture::new("image1")],
    properties: &[],
    projection: Projection::PerDraw,
    sources: Sources::NONE
        .wgsl(&wgsl::RASTER_SHADER)
        .msl(&msl::RASTER_SHADER)
        .glsl_es(&glsl_es::RASTER_SHADER)
        .glsl(&glsl::RASTER_SHADER),
};
