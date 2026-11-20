//! What the layer types' programs and shader sources share.
//!
//! A layer type's module has its programs (`programs`), the uniform blocks
//! its shaders read (`ubo`), and a module of sources for each shading
//! language: `wgsl` for `mln_webgpu`, `msl` for `mln_mtl`, `glsl_es` for
//! `mln_gl`, and `glsl` for `mln_vulkan` and `mln_d3d12`, which parse it
//! with `mln_glsl`. A language's sources build with the crate's feature of
//! that name (`glsl-es` for GLSL ES) and where its backends build: MSL on
//! Apple platforms, GLSL elsewhere but in the browser. Each holds the
//! program's code, which compiles after its backend's prelude, and the
//! tables of where the program's attributes, textures and buffers bind.

use mln_gfx::shaders::program::Property;

/// The paint property `u_<name>` of a program, whose values a layer binds
/// to attribute `attribute`.
pub(crate) const fn property(name: &'static str, attribute: usize) -> Property {
    Property { name, attribute }
}

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
        pub(crate) fn add_shader_sources(sources: &mut mln_gfx::shaders::source::ShaderSources) {
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

/// WGSL's tables. A texture at location L binds its sampler at L and the
/// texture at L + 1.
#[cfg(feature = "wgsl")]
pub(crate) mod wgsl {
    use mln_gfx::shaders::source::wgsl::{Attribute, Texture};

    pub(crate) const fn attr(index: usize, id: usize) -> Attribute {
        Attribute { index, id }
    }

    pub(crate) const fn tex(index: usize, id: usize) -> Texture {
        Texture { index, id, filterable: true }
    }

    pub(crate) const fn unfilterable_tex(index: usize, id: usize) -> Texture {
        Texture { index, id, filterable: false }
    }
}

/// MSL's tables, which describe the vertex layout the renderer produces:
/// instanced fill extrusions. The vertex buffers follow the shader's UBOs:
/// the layout vertices at `<layer>UBOCount`, then the fill extrusions'
/// instances, then the interleaved data-driven vertices, then the fill
/// extrusions' instances again, one vertex on, for each instance's next
/// outline vertex.
#[cfg(all(feature = "msl", target_vendor = "apple"))]
pub(crate) mod msl {
    use mln_gfx::gfx_types::AttributeDataType;
    use mln_gfx::shaders::source::msl::{Attribute, Texture};

    pub(crate) const fn attr(index: usize, data_type: AttributeDataType, buffer_index: usize, id: usize) -> Attribute {
        Attribute { index, data_type, buffer_index, id }
    }

    pub(crate) const fn tex(index: usize, id: usize) -> Texture {
        Texture { index, id }
    }
}

/// GLSL ES, which started as the C++ core's (`include/mln/shaders/gl/`), with
/// the port's changes, so that it reads the vertices its renderer makes and
/// draws what its WGSL does:
/// - `atlas_pos` (`mln_gl`'s fragment prelude) places the samples of a
///   pattern, and the pattern rectangles reach the fragment shaders `flat`.
/// - The fill extrusion shaders declare `gl_Position` invariant. Their vertex
///   shaders draw the roofs, and the instanced ones, which C++ has no GLSL
///   for, the walls: an instance of a quad per outline vertex, as the WGSL
///   does (C++'s GLSL draws both from one vertex buffer with normals).
/// - The instanced fill outline shaders, which C++ doesn't have, draw an
///   outline where the context draws no 2 px lines: a quad per segment, two
///   pixels wide as a wide line is, with the line shaders' fragment shaders.
/// - The fill, fill outline, fill extrusion and line pattern fragment shaders
///   set `precision highp float` on GL ES, as GL JS's do: overzoomed past a
///   source's maxzoom, their pattern positions outgrow a 16-bit mediump
///   float (maplibre/maplibre-native#4951). The background pattern draws at
///   the current zoom, so its positions stay small.
/// - Fragment shaders declare the uniform blocks they share with their
///   vertex shader first, in the vertex shader's order, which SwiftShader
///   needs (maplibre/maplibre-native#4625, and `line_gradient_sdf`).
///
/// The tables of where each shader's uniform blocks, attributes and textures
/// bind:
/// - The ids are `shader_defines`', so a shader's uniform blocks bind where
///   the renderer puts their buffers.
/// - A block's entry has the size of its UBO, which `mln_gl`'s compile test
///   checks against the program, and, where the renderer writes the block
///   for every drawable of a layer into one buffer, the size of that
///   buffer's elements.
/// - The fill extrusion shaders have the roofs' attributes, and the
///   instanced ones, which the renderer draws the walls with, the
///   instances'.
/// - `RasterShader`'s second texture has an id of its own.
/// - Every block a shader's GLSL declares has an entry, the prelude's
///   projection block too: WebGL2 rejects a draw whose program has a block
///   without a buffer, whether or not the shader reads it.
#[cfg(feature = "glsl-es")]
pub(crate) mod glsl_es {
    use mln_gfx::shaders::source::glsl_es::{Attribute, Texture, UniformBlock};

    /// A block whose UBO is a `T`.
    pub(crate) const fn block<T>(name: &'static str, id: usize) -> UniformBlock {
        UniformBlock { name, id, size: size_of::<T>(), element_size: 0 }
    }

    /// A block whose UBO is a `T`, one in each `Element` of a layer's
    /// buffer.
    pub(crate) const fn element<T, Element>(name: &'static str, id: usize) -> UniformBlock {
        assert!(size_of::<T>() <= size_of::<Element>());
        UniformBlock { name, id, size: size_of::<T>(), element_size: size_of::<Element>() }
    }

    pub(crate) const fn attr(name: &'static str, id: usize) -> Attribute {
        Attribute { name, id }
    }

    pub(crate) const fn tex(name: &'static str, id: usize) -> Texture {
        Texture { name, id }
    }
}

/// The GLSL that `mln_glsl` parses with naga's GLSL front end, which started
/// as the C++ core's Vulkan GLSL (`include/mln/shaders/vulkan/`), with the
/// port's changes beyond `mln_glsl`'s own:
/// - The evaluated properties of the fill, fill extrusion and color relief
///   layers bind one binding after the other layers'.
/// - A fill extrusion wall reads its outline vertex and the next one as
///   instance attributes, where C++ indexes a storage buffer of the outline
///   with `gl_InstanceIndex`. The pulled walls and outlines have no vertex
///   inputs: they read the bucket's vertices from storage buffers in the
///   drawable set, by `gl_VertexIndex`.
/// - The pattern rectangles reach the fragment stage flat.
///
/// The tables describe the vertex layout the renderer produces: the
/// instanced fill extrusions have the next outline vertex as instance
/// attributes of its own.
#[cfg(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))))]
pub(crate) mod glsl {
    use mln_gfx::gfx_types::AttributeDataType;
    use mln_gfx::shaders::source::glsl::{Attribute, Texture};

    pub(crate) const fn attr(index: usize, data_type: AttributeDataType, id: usize) -> Attribute {
        Attribute { index, data_type, id }
    }

    pub(crate) const fn tex(index: usize, id: usize) -> Texture {
        Texture { index, id }
    }
}
