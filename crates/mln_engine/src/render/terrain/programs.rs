//! The terrain's programs, and what the programs that stand on it read of
//! the ground.

use mln_gfx::gfx_types::AttributeDataType::{Float, Short4};
use mln_gfx::shaders::program::{Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Stages, Variants};
use mln_gfx::shaders::source::Sources;

use super::ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, TerrainDrawableUBO, TerrainSurfaceUBO};
use super::{glsl, glsl_es, msl, wgsl};

/// Over terrain, the height of the ground under a feature's anchor, in
/// meters with the exaggeration, which the terrain variants of the programs
/// that stand on it read.
pub const GROUND_HEIGHT: Attribute = Attribute::new("ground_height", Float).only_in(Variants::Standing);

/// The ground's depth: each pixel's eye distance to the nearest ground drawn
/// there, the bits of an `f32`, least significant byte first, in an RGBA8
/// texel; 0 where none is. The terrain variants of the programs that hide
/// behind the ground read it, in the slot their own textures leave free.
pub const GROUND_DEPTH: ProgramTexture =
    ProgramTexture::texels("ground_depth", Stages::Vertex).only_in(Variants::Standing);

/// The DEM: RGBA8 texels behind a one-texel border, which the vertex stage
/// reads texel by texel, without a sampler. It is at slot 2, which the
/// sources name, so that a program that reads the ground keeps slots 0 and 1
/// for its own textures.
pub const GROUND_DEM: ProgramTexture = ProgramTexture::texels("ground_dem", Stages::Vertex);

const TERRAIN_DRAWABLE: ProgramBuffer = ProgramBuffer::storage::<TerrainDrawableUBO>("drawableVector", Stages::Vertex);

/// How the terrain's surface stands, and its fog.
pub const TERRAIN_SURFACE: ProgramBuffer = ProgramBuffer::uniform::<TerrainSurfaceUBO>("terrain", 0, Stages::Vertex);

/// The terrain's surface: a grid over a tile, raised by its DEM, with the
/// tile's drape over it, which the fragment stage samples.
pub static TERRAIN: Program = Program {
    name: "TerrainShader",
    buffers: &[
        TERRAIN_DRAWABLE,
        ProgramBuffer::storage::<HillshadeTilePropsUBO>("lightVector", Stages::Fragment),
        ProgramBuffer { stages: Stages::Both, ..TERRAIN_SURFACE },
        ProgramBuffer::uniform::<HillshadeEvaluatedPropsUBO>("props", 1, Stages::Fragment),
    ],
    paint_params: None,
    projection: Projection::PerDraw,
    // A grid vertex: x and y in tile units, whether it is a skirt's lower
    // edge, and 0.
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::new("drape", Stages::Fragment), GROUND_DEM],
    properties: &[],
    sources: Sources::NONE.wgsl(&wgsl::TERRAIN).msl(&msl::TERRAIN).glsl(&glsl::TERRAIN).glsl_es(&glsl_es::TERRAIN),
};

/// The ground's depth: the terrain's surface, as [`TERRAIN`] raises it,
/// writing each pixel's eye distance, which the layers that stand on
/// terrain read to hide behind the ground in front of them.
pub static TERRAIN_DEPTH: Program = Program {
    name: "TerrainDepthShader",
    buffers: &[TERRAIN_DRAWABLE, TERRAIN_SURFACE],
    paint_params: None,
    projection: Projection::PerDraw,
    attributes: &[Attribute::new("pos", Short4)],
    instance_attributes: &[],
    textures: &[ProgramTexture::UNUSED, ProgramTexture::UNUSED, GROUND_DEM],
    properties: &[],
    sources: Sources::NONE
        .wgsl(&wgsl::TERRAIN_DEPTH)
        .msl(&msl::TERRAIN_DEPTH)
        .glsl(&glsl::TERRAIN_DEPTH)
        .glsl_es(&glsl_es::TERRAIN_DEPTH),
};
