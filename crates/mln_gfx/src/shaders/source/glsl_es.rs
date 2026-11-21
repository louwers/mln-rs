//! GLSL ES 3.00, which `mln_gl` compiles, as `#version 330 core` on desktop
//! OpenGL.
//!
//! Every uniform block a program's GLSL declares, the prelude's too, is a
//! buffer of the program's or the engine's, which a draw binds: WebGL2
//! rejects a draw whose program has a block without a buffer, whether or
//! not the shader reads it.

use std::fmt::Write as _;

use crate::shaders::block::{Block, Member, MemberType};

/// The GLSL of a program.
///
/// A stage compiles from `#version 300 es`, or `#version 330 core` on
/// desktop OpenGL, the program's defines, the backend's prelude of that
/// stage, the view's block (`ViewUBO`) where the stage names one of its
/// members, and the shader's.
#[derive(Debug)]
pub struct Glsl {
    /// The vertex shader.
    pub vertex: &'static str,
    /// The fragment shader.
    pub fragment: &'static str,
}

/// The declaration of `block` in GLSL ES: a `std140` uniform block whose
/// members are in the program's scope, named as [`Block`] says.
#[must_use]
pub fn declaration(block: &Block) -> String {
    let mut text = format!("layout (std140) uniform {} {{\n", block.name);
    for member in block.members {
        let (ty, array) = match member.ty {
            MemberType::Float => ("float", None),
            MemberType::Int => ("int", None),
            MemberType::Uint => ("uint", None),
            MemberType::Bool => ("bool", None),
            MemberType::Vec2 => ("vec2", None),
            MemberType::Vec3 => ("vec3", None),
            MemberType::Vec4 => ("vec4", None),
            MemberType::Mat4 => ("mat4", None),
            MemberType::Vec4Array(n) => ("vec4", Some(n)),
        };
        let name = member_name(block, member);
        let array = array.map(|n| format!("[{n}]")).unwrap_or_default();
        // A `bool` has no precision.
        let precision =
            if member.ty == MemberType::Bool { String::new() } else { format!("{} ", member.precision.glsl_es()) };
        _ = writeln!(text, "    {precision}{ty} {name}{array};");
    }
    text + "};\n"
}

/// The name of `member` of `block` in GLSL ES, in the program's scope.
#[must_use]
pub fn member_name(block: &Block, member: &Member) -> String {
    if member.is_padding() {
        format!("{}_{}", block.name, member.name)
    } else {
        format!("{}{}", block.glsl_es_prefix, member.name)
    }
}
