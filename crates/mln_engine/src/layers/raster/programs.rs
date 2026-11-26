//! The rasters' program.

use mln_gfx::shaders::program::{Program, ProgramBuffer, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{glsl, glsl_es, msl, wgsl};
use crate::shaders::programs::{POS, TEXTURE_POS};

pub static RASTER: Program = Program {
    name: "RasterShader",
    buffers: &[
        ProgramBuffer::storage::<RasterDrawableUBO>("drawableVector", Stages::Vertex),
        ProgramBuffer::uniform::<RasterEvaluatedPropsUBO>("props", 0, Stages::Both),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[POS, TEXTURE_POS],
    instance_attributes: &[],
    textures: &[ProgramTexture::new("image0", Stages::Fragment), ProgramTexture::new("image1", Stages::Fragment)],
    properties: &[],
    sources: Sources::NONE.wgsl(&wgsl::RASTER).msl(&msl::RASTER).glsl(&glsl::RASTER).glsl_es(&glsl_es::RASTER),
};
