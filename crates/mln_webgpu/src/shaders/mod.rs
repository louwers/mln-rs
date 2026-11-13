//! The WebGPU shaders: their WGSL and its preprocessor, and the shader
//! sources, programs and groups.

pub(crate) mod fill_extrusion_wgsl;
pub(crate) mod shader_group;
pub(crate) mod shader_program;
pub(crate) mod shader_source;
pub(crate) mod wgsl_preprocessor;
/// The C++ core's WGSL, with the port's changes.
pub(crate) mod wgsl_sources;
