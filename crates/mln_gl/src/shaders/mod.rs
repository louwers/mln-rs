//! The OpenGL shaders: their GLSL, sources, programs and shader groups.

/// The C++ core's GLSL, with the port's changes.
#[rustfmt::skip]
pub(crate) mod glsl_sources;
pub(crate) mod shader_group;
pub(crate) mod shader_program;
pub(crate) mod shader_source;
