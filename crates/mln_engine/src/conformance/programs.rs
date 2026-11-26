//! The programs that only the conformance cases draw with.

use mln_gfx::gfx_types::AttributeDataType::{Float4, Short4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramTexture, Projection, Stages};
use mln_gfx::shaders::source::Sources;

use super::{glsl, glsl_es, msl, wgsl};
use crate::render::terrain::programs::{GROUND_DEM, TERRAIN_SURFACE};

/// For the backend conformance cases: quads whose vertex stage reads their
/// color from a texture, by texel. The renderer doesn't draw with it.
pub static VERTEX_TEXEL: Program = Program {
    name: "VertexTexelShader",
    buffers: &[],
    paint_params: None,
    projection: Projection::None,
    attributes: &[Attribute::new("pos_texel", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::texels("image", Stages::Vertex)],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::VERTEX_TEXEL)
        .msl(&msl::VERTEX_TEXEL)
        .glsl(&glsl::VERTEX_TEXEL)
        .glsl_es(&glsl_es::VERTEX_TEXEL),
};

/// For the backend conformance cases: probes whose vertex stage reads a
/// height by the terrain's height rule, and whose fragment stage writes its
/// bits. The renderer doesn't draw with it.
pub static TERRAIN_PROBE: Program = Program {
    name: "TerrainProbeShader",
    buffers: &[TERRAIN_SURFACE],
    paint_params: None,
    projection: Projection::None,
    attributes: &[
        // Where a probe draws, in clip x and y, then where it reads the
        // height, in tile units.
        Attribute::new("pos", Float4),
        // Where the probe reads its DEM: the scale and offset that map the
        // tile's uv to the DEM's (`uv * scale + offset`), then the DEM's
        // width in pixels, 0 for flat ground.
        Attribute::new("dem_coords", Float4),
    ],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::TERRAIN_PROBE)
        .msl(&msl::TERRAIN_PROBE)
        .glsl(&glsl::TERRAIN_PROBE)
        .glsl_es(&glsl_es::TERRAIN_PROBE),
};
