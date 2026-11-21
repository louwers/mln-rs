//! Shader sources as data, by shading language: what a backend compiles a
//! program from.
//!
//! Each shading language a backend compiles has a module, whose `Source`
//! holds a program's code in it and the tables of where the program's
//! attributes, textures and buffers bind: [`wgsl`] for `mln_webgpu`, [`msl`]
//! for `mln_mtl`, [`glsl_es`] for `mln_gl`, and [`glsl`] for the backends
//! that compile through `mln_glsl`, `mln_vulkan` and `mln_d3d12`. The
//! languages are a closed set: [`ShaderSource`] is sealed, so a new one is a
//! change to this crate.
//!
//! This is the program and shader source contract. A layer type declares
//! each program it draws with as a `static` [`Program`], whose buffers,
//! textures and attributes bind at ids it chooses within the ranges of
//! [`shader_defines`](crate::shaders::shader_defines): uniform blocks below
//! `MAX_UBO_COUNT_PER_SHADER`, the frame's global ones at `ID_GLOBAL_*`, a
//! draw's own at the drawable-reserved ids (`ID_DRAWABLE_RESERVED_*`,
//! `ID_PROJECTION_UBO`) and the layer's from `DRAWABLE_RESERVED_UBO_COUNT`
//! on; vertex buffers that the shader reads as storage at and past
//! `MAX_UBO_COUNT_PER_SHADER`; and textures and attributes below
//! `MAX_TEXTURE_COUNT_PER_SHADER` and `MAX_ATTRIBUTE_COUNT_PER_SHADER`.
//! The type gives its sources in the languages it has them in through a [`ShaderSources`]
//! (`mln_render`'s `LayerType::add_shader_sources`, which registering the
//! type calls). A renderer adds its registry's sources to its `Gpu`
//! ([`Gpu::add_shader_sources`](crate::gpu::Gpu::add_shader_sources)), which
//! takes those of its own language; a backend compiles the engine's own
//! programs from a table of its own. The backend compiles each variant of a
//! program that a draw needs from its source; compiling a program without
//! a source in its language fails with [`GpuError::Unsupported`]. Sources
//! are `'static` data, which every thread's renderers share.

pub mod glsl;
pub mod glsl_es;
pub mod msl;
pub mod wgsl;

use mln_util::hash::HashMap;

use crate::gpu::GpuError;
use crate::shaders::program::Program;

/// A program's source in one shading language: [`wgsl::Source`],
/// [`msl::Source`], [`glsl_es::Source`] or [`glsl::Source`].
pub trait ShaderSource: Sync + sealed::Language + 'static {}

mod sealed {
    use super::{ShaderSources, Sources};

    pub trait Language: Sized + 'static {
        fn of(sources: &ShaderSources) -> &Sources<Self>;
        fn of_mut(sources: &mut ShaderSources) -> &mut Sources<Self>;
    }
}

macro_rules! shader_source {
    ($language:ident) => {
        impl sealed::Language for $language::Source {
            fn of(sources: &ShaderSources) -> &Sources<Self> {
                &sources.$language
            }

            fn of_mut(sources: &mut ShaderSources) -> &mut Sources<Self> {
                &mut sources.$language
            }
        }

        impl ShaderSource for $language::Source {}
    };
}

shader_source!(wgsl);
shader_source!(msl);
shader_source!(glsl_es);
shader_source!(glsl);

/// The source of type `S` of each program that has one.
pub struct Sources<S: 'static> {
    sources: HashMap<&'static Program, &'static S>,
}

impl<S> Default for Sources<S> {
    fn default() -> Self {
        Sources { sources: HashMap::default() }
    }
}

impl<S> Sources<S> {
    /// The sources of `table`.
    #[must_use]
    pub fn new(table: &[(&'static Program, &'static S)]) -> Sources<S> {
        let mut sources = Sources::default();
        sources.extend(table.iter().copied());
        sources
    }

    /// Adds each source of `sources`, in place of any its program had.
    pub fn extend(&mut self, sources: impl IntoIterator<Item = (&'static Program, &'static S)>) {
        self.sources.extend(sources);
    }

    /// The source of `program`. Fails with [`GpuError::Unsupported`] if it
    /// has none.
    pub fn get(&self, program: &'static Program) -> Result<&'static S, GpuError> {
        self.sources
            .get(program)
            .copied()
            .ok_or_else(|| GpuError::Unsupported(format!("no source for {}", program.name)))
    }

    /// Each program and its source, in no order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static Program, &'static S)> + '_ {
        self.sources.iter().map(|(&program, &source)| (program, source))
    }
}

/// The sources of programs, in each shading language they have one in.
#[derive(Default)]
pub struct ShaderSources {
    wgsl: Sources<wgsl::Source>,
    msl: Sources<msl::Source>,
    glsl_es: Sources<glsl_es::Source>,
    glsl: Sources<glsl::Source>,
}

impl ShaderSources {
    /// Adds the source of each program of `table`, in place of any it had
    /// in that language.
    pub fn add<S: ShaderSource>(&mut self, table: &[(&'static Program, &'static S)]) {
        <S as sealed::Language>::of_mut(self).extend(table.iter().copied());
    }

    /// The sources in the language of `S`.
    #[must_use]
    pub fn of<S: ShaderSource>(&self) -> &Sources<S> {
        <S as sealed::Language>::of(self)
    }
}

impl std::fmt::Debug for ShaderSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn names<S>(sources: &Sources<S>) -> Vec<&'static str> {
            sources.iter().map(|(program, _)| program.name).collect()
        }
        f.debug_struct("ShaderSources")
            .field("wgsl", &names(&self.wgsl))
            .field("msl", &names(&self.msl))
            .field("glsl_es", &names(&self.glsl_es))
            .field("glsl", &names(&self.glsl))
            .finish()
    }
}
