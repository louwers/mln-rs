//! The GLSL that `mln_glsl` parses with naga's GLSL front end and the
//! Vulkan and D3D12 backends write as SPIR-V and HLSL, whose buffers and
//! textures bind at `mln_glsl`'s descriptor set layout.

use crate::shaders::block::{Block, MemberType, c_like_members};

/// The GLSL of a program.
///
/// A stage compiles from the structs of the engine's blocks, `mln_glsl`'s
/// prelude of the stage, the structs of the program's blocks, `prelude` and
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

/// The declaration of `block` in the GLSL: a struct of its members, padded
/// to `size` bytes, the stride of the storage buffer whose elements it is,
/// or its own size. A buffer of it is a `std140` block of one member of the
/// struct, or of an array of them.
#[must_use]
pub fn declaration(block: &Block, size: usize) -> String {
    let members = c_like_members(
        block,
        size,
        |ty| match ty {
            MemberType::Float => "float",
            MemberType::Int | MemberType::Bool => "int",
            MemberType::Uint => "uint",
            MemberType::Vec2 => "vec2",
            MemberType::Vec3 => "vec3",
            MemberType::Vec4 | MemberType::Vec4Array(_) => "vec4",
            MemberType::Mat4 => "mat4",
        },
        "vec4",
    );
    format!("struct {} {{\n{members}}};\n", block.name)
}
