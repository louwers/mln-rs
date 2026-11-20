//! Shader sources as data, by shading language: what a backend compiles a
//! program from.
//!
//! Each shading language a backend compiles has a module, whose `Source`
//! holds a program's code in it and the tables of where the program's
//! attributes, textures and buffers bind: [`wgsl`] for `mln_webgpu`, [`msl`]
//! for `mln_mtl`, [`glsl_es`] for `mln_gl`, and [`glsl`] for the backends
//! that compile through `mln_glsl`, `mln_vulkan` and `mln_d3d12`.

pub mod glsl;
pub mod glsl_es;
pub mod msl;
pub mod wgsl;

use mln_util::hash::HashMap;

use crate::gpu::GpuError;
use crate::shaders::program::Program;

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
}
