//! The OpenGL shaders: their GLSL, sources, programs and shader groups.

/// The C++ core's GLSL, with the port's changes.
#[rustfmt::skip]
pub mod glsl_sources;
pub mod shader_group;
pub mod shader_program;
pub mod shader_source;
