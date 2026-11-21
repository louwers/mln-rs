//! GLSL ES 3.00, which `mln_gl` compiles, as `#version 330 core` on desktop
//! OpenGL.
//!
//! Every uniform block a program's GLSL declares, the prelude's too, is a
//! buffer of the program's or the engine's, which a draw binds: WebGL2
//! rejects a draw whose program has a block without a buffer, whether or
//! not the shader reads it.

/// The GLSL of a program.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the backend's prelude of that
/// stage and the shader's.
#[derive(Debug)]
pub struct Glsl {
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}
