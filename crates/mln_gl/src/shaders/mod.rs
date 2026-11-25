//! The OpenGL shaders: the engine's GLSL ES prelude, and the declarations
//! written from the programs, which precede their GLSL ES.

pub(crate) mod declarations;
pub(crate) mod precision;
/// The engine's GLSL ES helpers.
#[rustfmt::skip]
pub(crate) mod prelude;
pub(crate) mod shader_source;
