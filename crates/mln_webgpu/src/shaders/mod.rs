//! The WebGPU shaders: their WGSL and its preprocessor, and the shader
//! sources, programs and groups.

pub mod fill_extrusion_wgsl;
pub mod shader_group;
pub mod shader_program;
pub mod shader_source;
pub mod wgsl_preprocessor;
/// The C++ core's WGSL, with the port's changes.
pub mod wgsl_sources;
