//! The Metal shaders: their MSL, and the tables of their inputs and
//! textures.

#[rustfmt::skip]
pub(crate) mod msl_sources;
pub(crate) mod shader_source;

#[cfg(test)]
mod msl_compile_test;
