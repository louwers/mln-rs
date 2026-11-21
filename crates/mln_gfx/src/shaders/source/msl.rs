//! MSL, which `mln_mtl` compiles.

use crate::shaders::block::{Block, MemberType, c_like_members};

/// The MSL of a program. Its library compiles from the backend's prelude,
/// the structs of the engine's blocks and the program's, `prelude` and
/// `source`.
#[derive(Debug)]
pub struct Msl {
    /// The name of the vertex function.
    pub vertex_main_function: &'static str,
    /// The name of the fragment function.
    pub fragment_main_function: &'static str,
    /// The shader's own constants and structs, "" if none.
    pub prelude: &'static str,
    /// The functions.
    pub source: &'static str,
}

/// The declaration of `block` in MSL: a struct of its members, aligned to
/// 16 bytes and padded to `size` bytes, the stride of the storage buffer
/// whose elements it is, or its own size, which a `static_assert` checks.
#[must_use]
pub fn declaration(block: &Block, size: usize) -> String {
    let members = c_like_members(
        block,
        size,
        |ty| match ty {
            MemberType::Float => "float",
            MemberType::Int | MemberType::Bool => "int",
            MemberType::Uint => "uint",
            MemberType::Vec2 => "float2",
            MemberType::Vec3 => "packed_float3",
            MemberType::Vec4 | MemberType::Vec4Array(_) => "float4",
            MemberType::Mat4 => "float4x4",
        },
        "float4",
    );
    let name = block.name;
    format!("struct alignas(16) {name} {{\n{members}}};\nstatic_assert(sizeof({name}) == {size}, \"wrong size\");\n")
}
