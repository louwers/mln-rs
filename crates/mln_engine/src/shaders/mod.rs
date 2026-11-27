//! The engine's programs: its own ([`programs`]), the blocks they read
//! ([`ubo`]) and their text in each language, and every program it draws
//! with ([`PROGRAMS`]): those and the terrain's ([`terrain`]). A layer
//! type's programs are in its own module.

// The built-in programs' WGSL and MSL started as MapLibre Native's, and
// read the vertices and blocks this renderer makes.
//
// Their GLSL is the C++ core's, copied from the raw string literals of
//   include/mln/shaders/vulkan/background.hpp
//   include/mln/shaders/vulkan/circle.hpp
//   include/mln/shaders/vulkan/clipping_mask.hpp
//   include/mln/shaders/vulkan/collision.hpp
//   include/mln/shaders/vulkan/color_relief.hpp
//   include/mln/shaders/vulkan/common.hpp
//   include/mln/shaders/vulkan/fill.hpp
//   include/mln/shaders/vulkan/fill_extrusion.hpp
//   include/mln/shaders/vulkan/globe_depth.hpp
//   include/mln/shaders/vulkan/heatmap.hpp
//   include/mln/shaders/vulkan/heatmap_texture.hpp
//   include/mln/shaders/vulkan/hillshade.hpp
//   include/mln/shaders/vulkan/hillshade_prepare.hpp
//   include/mln/shaders/vulkan/line.hpp
//   include/mln/shaders/vulkan/raster.hpp
//   include/mln/shaders/vulkan/sky.hpp
//   include/mln/shaders/vulkan/symbol.hpp
// with the port's changes, so that it reads the vertices and UBOs its
// renderer makes, and compiles with naga's GLSL front end:
// - The stages read what their programs declare (`mln_glsl`'s declarations)
//   by its names there. There is no `GlobalPlatformParamsUBO`.
// - A texture and its sampler are two bindings, `<name>_texture` and
//   `<name>_sampler`, combined where they are sampled.
// - Symbols read vertex attributes, with the port's `tex_origin` and
//   `height_offset`, where C++ reads per-instance storage buffers.
// - A fill extrusion wall reads its outline vertex and the next one as
//   instance attributes, where C++ indexes a storage buffer of the outline
//   with `gl_InstanceIndex`. The port's pulled walls and outlines have no
//   vertex inputs: they read the bucket's vertices from storage buffers in
//   the drawable set, by `gl_VertexIndex`.
// - Atlas positions go through `atlas_pos`, and the pattern rectangles reach
//   the fragment stage flat.
// - On the globe, a symbol vertex stage puts a screen-aligned label whose
//   anchor is behind the horizon outside the clip volume, unless the label
//   is raised.
// - On the globe, points on the sphere are offsets from the tile's anchor,
//   which the projection matrix carries, so float32 keeps them exact close
//   in (`projectToSphere`).
// - What naga doesn't take is written another way: no precision qualifiers,
//   `int` for the `bool` members of buffers, no sampler parameters, and
//   `flat` on an integer output.
// - Clip space is y-up, as in OpenGL, Metal, D3D and naga's IR: no stage
//   flips Y (`applySurfaceTransform`), the SPIR-V back end does for Vulkan.
//
// Their GLSL ES is the C++ core's, copied from the raw string literals of
//   include/mln/shaders/gl/atmosphere.hpp
//   include/mln/shaders/gl/background.hpp
//   include/mln/shaders/gl/background_pattern.hpp
//   include/mln/shaders/gl/circle.hpp
//   include/mln/shaders/gl/clipping_mask.hpp
//   include/mln/shaders/gl/color_relief.hpp
//   include/mln/shaders/gl/fill.hpp
//   include/mln/shaders/gl/fill_extrusion.hpp
//   include/mln/shaders/gl/fill_extrusion_instanced.hpp
//   include/mln/shaders/gl/fill_extrusion_pattern.hpp
//   include/mln/shaders/gl/fill_extrusion_pattern_instanced.hpp
//   include/mln/shaders/gl/fill_outline.hpp
//   include/mln/shaders/gl/fill_outline_pattern.hpp
//   include/mln/shaders/gl/fill_pattern.hpp
//   include/mln/shaders/gl/globe_depth.hpp
//   include/mln/shaders/gl/heatmap.hpp
//   include/mln/shaders/gl/heatmap_texture.hpp
//   include/mln/shaders/gl/hillshade.hpp
//   include/mln/shaders/gl/hillshade_prepare.hpp
//   include/mln/shaders/gl/line.hpp
//   include/mln/shaders/gl/line_gradient.hpp
//   include/mln/shaders/gl/line_gradient_sdf.hpp
//   include/mln/shaders/gl/line_pattern.hpp
//   include/mln/shaders/gl/line_sdf.hpp
//   include/mln/shaders/gl/prelude.hpp
//   include/mln/shaders/gl/raster.hpp
//   include/mln/shaders/gl/shader_group_gl.hpp
//   include/mln/shaders/gl/shader_info.hpp
//   include/mln/shaders/gl/shader_program_gl.hpp
//   include/mln/shaders/gl/sky.hpp
//   include/mln/shaders/gl/symbol_icon.hpp
//   include/mln/shaders/gl/symbol_sdf.hpp
//   include/mln/shaders/gl/symbol_text_and_icon.hpp
// with the port's changes, so that it reads the vertices its renderer makes
// and draws what its WGSL does:
// - `atlas_pos` (the fragment prelude) places the samples of an atlas image,
//   in the pattern and symbol shaders, and the pattern rectangles and the
//   symbols' atlas origins reach the fragment shaders `flat`.
// - The symbol shaders read `a_tex_origin`, the atlas origin of a quad's
//   image, and the symbol shaders read `a_height_offset`, the symbol's
//   `symbol-height-offset`, as GL JS's do.
// - On the globe, the symbol shaders drop a screen-aligned label whose anchor
//   is behind the horizon, unless the label is raised.
// - The fill extrusion shaders declare `gl_Position` invariant. Their vertex
//   shaders draw the roofs, and the instanced ones, which C++ has no GLSL
//   for, the walls: an instance of a quad per outline vertex, as the port's
//   WGSL does (C++'s GLSL draws both from one vertex buffer with normals).
// - The instanced fill outline shaders, which C++ doesn't have, draw an
//   outline where the context draws no 2 px lines: a quad per segment, two
//   pixels wide as a wide line is, with the line shaders' fragment shaders.
// - The fill, fill outline, fill extrusion and line pattern fragment shaders
//   set `precision highp float` on GL ES, as GL JS's do: overzoomed past a
//   source's maxzoom, their pattern positions outgrow a 16-bit mediump
//   float (maplibre/maplibre-native#4951). The background pattern draws at
//   the current zoom, so its positions stay small.
// - ClippingMaskProgram projects through the projection block as the other
//   shaders do, and has a globe variant.
// - On the globe, points on the sphere are offsets from the tile's anchor,
//   which the projection matrix carries, so float32 keeps them exact close
//   in (`projectToSphere`).
// - The terrain shaders have no C++ counterpart: they are the port's own, with
//   the WGSL's height rule.

#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
#[rustfmt::skip]
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

/// The terrain's programs and the blocks they read, which the terrain keeps
/// with its text.
pub mod terrain {
    pub use crate::render::terrain::{programs, ubo};
}

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::Program;

/// The programs the engine draws with: its own and the terrain's. Each
/// crate of layer types lists its own (in `mln_builtins`,
/// `mln_builtins::shaders::PROGRAMS`).
pub static PROGRAMS: &[&Program] = &[
    &programs::CLIPPING_MASK,
    &programs::GLOBE_DEPTH,
    &programs::SKY,
    &programs::ATMOSPHERE,
    &programs::LAYER_COMPOSITE,
    &crate::render::terrain::programs::TERRAIN,
    &crate::render::terrain::programs::TERRAIN_DEPTH,
];

/// The projection variants of a program: those that stand on terrain too,
/// for one that `stands`, a program of a layer type that stands on terrain
/// ([`OnTerrain::Stands`](crate::layer::OnTerrain::Stands)).
#[must_use]
pub fn projection_variants(stands: bool) -> &'static [ProjectionVariant] {
    const ALL: [ProjectionVariant; 4] = [
        ProjectionVariant::Mercator,
        ProjectionVariant::Globe,
        ProjectionVariant::Terrain,
        ProjectionVariant::GlobeTerrain,
    ];
    if stands { &ALL } else { &ALL[..2] }
}
