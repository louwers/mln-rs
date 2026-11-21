//! The GLSL that `mln_glsl` parses with naga's GLSL front end and the
//! Vulkan and D3D12 backends write as SPIR-V and HLSL, whose buffers and
//! textures bind at `mln_glsl`'s descriptor set layout.

/// The GLSL of a program.
///
/// A stage compiles from `mln_glsl`'s prelude of the stage, `prelude` and
/// the stage's source.
#[derive(Debug)]
pub struct Glsl {
    /// The shader's own constants (its buffer bindings), "" if none.
    pub prelude: &'static str,
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}
