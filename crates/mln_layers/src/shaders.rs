//! What the layer types' programs and shader sources share.
//!
//! A layer type's module has its programs (`programs`), the uniform blocks
//! its shaders read (`ubo`), and a module of sources for each shading
//! language: `wgsl` for `mln_webgpu`, `msl` for `mln_mtl`, `glsl_es` for
//! `mln_gl`, and `glsl` for `mln_vulkan` and `mln_d3d12`, which parse it
//! with `mln_glsl`. A language's sources build with the crate's feature of
//! that name (`glsl-es` for GLSL ES) and where its backends build: MSL on
//! Apple platforms, GLSL elsewhere but in the browser. Each holds the
//! program's code and the tables of where the program's attributes,
//! textures and buffers bind, which follow the rules of
//! `mln_gfx::shaders::source`'s module of the language. The ids in the
//! tables are the program's, in its `programs` module, and the engine's
//! reserved ones (`mln_gfx::shaders::shader_defines`), so a shader's
//! buffers bind where the renderer puts them.
//!
//! The MSL tables describe the vertex layout the renderer produces:
//! instanced fill extrusions. The vertex buffers follow the shader's UBOs:
//! the layout vertices at `<layer>UBOCount`, then the fill extrusions'
//! instances, then the interleaved data-driven vertices, then the fill
//! extrusions' instances again, one vertex on, for each instance's next
//! outline vertex.
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
//! - Fragment shaders declare the uniform blocks they share with their
//!   vertex shader first, in the vertex shader's order, which SwiftShader
//!   needs (maplibre/maplibre-native#4625, and `line_gradient_sdf`).
//! - The fill extrusion tables have the roofs' attributes, and the instanced
//!   ones, which the renderer draws the walls with, the instances'.
//! - `RasterShader`'s second texture has an id of its own.
//!
//! The GLSL that `mln_glsl` parses:
//! - The evaluated properties of the fill, fill extrusion and color relief
//!   layers bind one binding after the other layers'.
//! - A fill extrusion wall reads its outline vertex and the next one as
//!   instance attributes. The pulled walls and outlines have no vertex
//!   inputs: they read the bucket's vertices from storage buffers in the
//!   drawable set, by `gl_VertexIndex`.
//! - The pattern rectangles reach the fragment stage flat.

/// Declares the modules of a layer type's sources, one for each shading
/// language that the build has, as this module's docs say, and
/// `add_shader_sources(sources)`, which adds the `SOURCES` of each.
macro_rules! shader_sources {
    () => {
        #[cfg(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))))]
        mod glsl;
        #[cfg(feature = "glsl-es")]
        mod glsl_es;
        #[cfg(all(feature = "msl", target_vendor = "apple"))]
        mod msl;
        #[cfg(feature = "wgsl")]
        mod wgsl;

        /// Adds the sources of the type's programs in the shading languages
        /// of the build.
        #[cfg_attr(
            not(any(
                feature = "wgsl",
                feature = "glsl-es",
                all(feature = "msl", target_vendor = "apple"),
                all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))),
            )),
            expect(unused_variables, reason = "a build without a shading language adds no sources")
        )]
        fn add_shader_sources(sources: &mut mln_gfx::shaders::source::ShaderSources) {
            #[cfg(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))))]
            sources.add(glsl::SOURCES);
            #[cfg(feature = "glsl-es")]
            sources.add(glsl_es::SOURCES);
            #[cfg(all(feature = "msl", target_vendor = "apple"))]
            sources.add(msl::SOURCES);
            #[cfg(feature = "wgsl")]
            sources.add(wgsl::SOURCES);
        }
    };
}
pub(crate) use shader_sources;
