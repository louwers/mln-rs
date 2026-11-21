//! WGSL, which `mln_webgpu` compiles.

use std::fmt::Write as _;

use crate::shaders::block::{Block, MemberType};

/// The WGSL of a program: its stages, and the constants and structs of its
/// own that both stages read.
///
/// A stage compiles from the backend's prelude, the structs of the engine's
/// blocks and the program's, the declarations of the engine's buffers that
/// `prelude` or the stage names (`view`, `globalIndex` and
/// `projectionVector`), `prelude` and the stage's source.
#[derive(Debug)]
pub struct Wgsl {
    /// The vertex stage, or `None` for an empty one.
    pub vertex: Option<&'static str>,
    /// The fragment stage, or `None` for an empty one.
    pub fragment: Option<&'static str>,
    /// What both stages read, or `None` for nothing.
    pub prelude: Option<&'static str>,
}

/// The declaration of `block` in WGSL: a struct of its members, padded to
/// `size` bytes, the stride of the storage buffer whose elements it is, or
/// its own size.
#[must_use]
pub fn declaration(block: &Block, size: usize) -> String {
    let mut text = format!("struct {} {{\n", block.name);
    for member in block.members {
        let ty = match member.ty {
            MemberType::Float => "f32".to_owned(),
            MemberType::Int => "i32".to_owned(),
            MemberType::Uint | MemberType::Bool => "u32".to_owned(),
            MemberType::Vec2 => "vec2<f32>".to_owned(),
            MemberType::Vec3 => "vec3<f32>".to_owned(),
            MemberType::Vec4 => "vec4<f32>".to_owned(),
            MemberType::Mat4 => "mat4x4<f32>".to_owned(),
            MemberType::Vec4Array(n) => format!("array<vec4<f32>, {n}>"),
        };
        _ = writeln!(text, "    {}: {ty},", member.name);
    }
    if size > block.size {
        _ = writeln!(text, "    stride_pad: array<vec4<f32>, {}>,", (size - block.size) / 16);
    }
    text + "};\n"
}
