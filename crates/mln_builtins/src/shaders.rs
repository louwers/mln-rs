//! The built-in layer types' programs ([`PROGRAMS`]), and every program a
//! map of the built-ins draws with: those and the engine's
//! ([`every_program`]).

use mln_engine::shaders::projection_variants as variants;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::Program;

use crate::layers::{background, circle, color_relief, fill, fill_extrusion, heatmap, hillshade, line, raster, symbol};

/// The programs the built-in layer types draw with, and, with the
/// `gpu-conformance` feature, those the conformance cases draw with.
pub static PROGRAMS: &[&Program] = &[
    &background::programs::BACKGROUND,
    &background::programs::BACKGROUND_PATTERN,
    &circle::programs::CIRCLE,
    &fill::programs::FILL,
    &fill::programs::FILL_OUTLINE,
    &fill::programs::FILL_PATTERN,
    &fill::programs::FILL_OUTLINE_PATTERN,
    &fill::programs::FILL_OUTLINE_INSTANCED,
    &fill::programs::FILL_OUTLINE_PULLED,
    &fill::programs::FILL_OUTLINE_PATTERN_INSTANCED,
    &line::programs::LINE,
    &line::programs::LINE_GRADIENT,
    &line::programs::LINE_PATTERN,
    &line::programs::LINE_SDF,
    &line::programs::LINE_GRADIENT_SDF,
    &raster::programs::RASTER,
    &fill_extrusion::programs::FILL_EXTRUSION,
    &fill_extrusion::programs::FILL_EXTRUSION_INSTANCED,
    &fill_extrusion::programs::FILL_EXTRUSION_WALL,
    &fill_extrusion::programs::FILL_EXTRUSION_PULLED,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN_INSTANCED,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN_WALL,
    &heatmap::programs::HEATMAP,
    &heatmap::programs::HEATMAP_TEXTURE,
    &hillshade::programs::HILLSHADE,
    &hillshade::programs::HILLSHADE_PREPARE,
    &color_relief::programs::COLOR_RELIEF,
    &symbol::programs::SYMBOL_ICON,
    &symbol::programs::SYMBOL_SDF,
    &symbol::programs::SYMBOL_TEXT_AND_ICON,
    #[cfg(feature = "gpu-conformance")]
    &crate::conformance::programs::VERTEX_TEXEL,
    #[cfg(feature = "gpu-conformance")]
    &crate::conformance::programs::TERRAIN_PROBE,
];

/// Those of [`PROGRAMS`] with variants that stand on terrain: the programs
/// of the built-in layer types that stand on it.
pub static STANDING: &[&Program] = &[
    &circle::programs::CIRCLE,
    &heatmap::programs::HEATMAP,
    &fill_extrusion::programs::FILL_EXTRUSION,
    &fill_extrusion::programs::FILL_EXTRUSION_INSTANCED,
    &fill_extrusion::programs::FILL_EXTRUSION_WALL,
    &fill_extrusion::programs::FILL_EXTRUSION_PULLED,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN_INSTANCED,
    &fill_extrusion::programs::FILL_EXTRUSION_PATTERN_WALL,
    &symbol::programs::SYMBOL_ICON,
    &symbol::programs::SYMBOL_SDF,
    &symbol::programs::SYMBOL_TEXT_AND_ICON,
];

/// Every program a map of the built-ins draws with: the engine's
/// (`mln_engine::shaders::PROGRAMS`), then the built-in layer types'.
pub fn every_program() -> impl Iterator<Item = &'static Program> {
    mln_engine::shaders::PROGRAMS.iter().chain(PROGRAMS).copied()
}

/// Whether `program`, one of [`every_program`], has variants that stand on
/// terrain: the engine's programs have none.
#[must_use]
pub fn stands(program: &Program) -> bool {
    STANDING.contains(&program)
}

/// The projection variants of `program`, one of [`every_program`].
#[must_use]
pub fn projection_variants(program: &Program) -> &'static [ProjectionVariant] {
    variants(stands(program))
}
