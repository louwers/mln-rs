//! The WebGPU shaders: the engine's WGSL prelude and the preprocessor, the
//! declarations written from the programs, and the programs compiled from
//! their WGSL.

pub(crate) mod declaration;
pub(crate) mod prelude;
pub(crate) mod shader_program;
pub(crate) mod wgsl_preprocessor;
