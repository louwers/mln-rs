//! The Metal shaders: their MSL, sources, programs and shader groups.

/// The C++ core's MSL, with the port's changes.
#[rustfmt::skip]
pub mod msl_sources;
pub mod shader_group;
pub mod shader_program;
pub mod shader_source;
