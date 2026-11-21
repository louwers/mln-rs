//! What the layer types' programs and shader sources share.
//!
//! A layer type's module has its programs (`programs`), each a `static`
//! `Program` that declares what its shaders bind and carries their sources,
//! the uniform blocks its shaders read (`ubo`), each declared once with
//! `mln_gfx::uniform_block!`, which the backends declare in each language
//! for the programs that read them, and a module of sources for
//! each shading language: `wgsl` for `mln_webgpu`, `msl` for `mln_mtl`,
//! `glsl_es` for `mln_gl`, and `glsl` for `mln_vulkan` and `mln_d3d12`,
//! which parse it with `mln_glsl`. A source binds what its program declares where
//! `mln_gfx::shaders::program::Program` says, and reads the program's
//! data-driven properties through the directives that
//! `mln_gfx::shaders::source` describes.
//!
//! The vertex buffers that the MSL reads follow the shader's buffers, from
//! the index after its last one: the layout vertices, then the fill
//! extrusions' instances, then the interleaved data-driven vertices, then
//! the fill extrusions' instances again, one vertex on, for each instance's
//! next outline vertex.
//!
//! The GLSL ES reads the vertices its renderer makes and draws what the
//! WGSL does:
//! - `atlas_pos` (`mln_gl`'s fragment prelude) places the samples of a
//!   pattern, and the pattern rectangles reach the fragment shaders `flat`.
//! - The fill extrusion shaders declare `gl_Position` invariant. Their vertex
//!   shaders draw the roofs, and the instanced ones the walls: an instance of
//!   a quad per outline vertex, as the WGSL does.
//! - The instanced fill outline shaders draw an outline where the context
//!   draws no 2 px lines: a quad per segment, two pixels wide as a wide line
//!   is, with the line shaders' fragment shaders.
//! - The fill, fill outline, fill extrusion and line pattern fragment shaders
//!   set `precision highp float` on GL ES, as GL JS's do: overzoomed past a
//!   source's maxzoom, their pattern positions outgrow a 16-bit mediump
//!   float (maplibre/maplibre-native#4951). The background pattern draws at
//!   the current zoom, so its positions stay small.
//! - The fill extrusion programs have the roofs' attributes, and the
//!   instanced ones, which the renderer draws the walls with, the instances'.
//!
//! The GLSL that `mln_glsl` parses:
//! - The evaluated properties of the fill, fill extrusion and color relief
//!   layers bind one binding after the other layers'.
//! - A fill extrusion wall reads its outline vertex and the next one as
//!   instance attributes. The pulled walls and outlines have no vertex
//!   inputs: they read the bucket's vertices from storage buffers in the
//!   drawable set, by `gl_VertexIndex`.
//! - The pattern rectangles reach the fragment stage flat.
