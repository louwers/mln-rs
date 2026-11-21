//! Uniform blocks, each declared once with [`uniform_block!`](crate::uniform_block):
//! the Rust struct a layer writes, and the layout that each backend
//! declares in its shading language for the programs that read it.

use std::fmt::Write as _;

/// The layout of a uniform block, or of an element of a storage buffer,
/// that [`uniform_block!`](crate::uniform_block) declares with its Rust
/// struct, the [`UniformBlock`] that has it.
///
/// A backend declares the block in its shading language with the members
/// in this order, at these offsets: a struct of the block's name in WGSL,
/// MSL and the Vulkan/D3D12 GLSL, and in GLSL ES a `std140` uniform block of
/// that name whose members are in the program's scope, each as
/// [`glsl_es_prefix`](Block::glsl_es_prefix) and its name, and each padding
/// member, one whose name starts with `pad`, as the block's name, `_` and
/// its name.
#[derive(Debug)]
pub struct Block {
    /// Its name in every shading language.
    pub name: &'static str,
    /// Its size in bytes, a multiple of 16.
    pub size: usize,
    /// Its members, in order.
    pub members: &'static [Member],
    /// What GLSL ES names its members with before their names, `u_` unless
    /// the declaration says otherwise.
    pub glsl_es_prefix: &'static str,
}

/// A member of a [`Block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: &'static str,
    pub ty: MemberType,
    /// Its offset in bytes in the block.
    pub offset: usize,
    /// Its precision in GLSL ES.
    pub precision: Precision,
}

/// The type of a [`Member`], and the Rust type of its field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberType {
    /// `f32`.
    Float,
    /// `i32`.
    Int,
    /// `u32`.
    Uint,
    /// [`Bool32`].
    Bool,
    /// `[f32; 2]`.
    Vec2,
    /// `[f32; 3]`, 12 bytes at a multiple of 16.
    Vec3,
    /// `[f32; 4]`.
    Vec4,
    /// `[f32; 16]`, a column-major 4 by 4 matrix.
    Mat4,
    /// `[[f32; 4]; n]`.
    Vec4Array(usize),
}

/// The precision of a member in GLSL ES, which the other shading languages
/// don't declare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    High,
    Medium,
    Low,
}

impl Precision {
    /// Its qualifier in GLSL ES.
    #[must_use]
    pub const fn glsl_es(self) -> &'static str {
        match self {
            Precision::High => "highp",
            Precision::Medium => "mediump",
            Precision::Low => "lowp",
        }
    }
}

impl Member {
    /// Whether it is padding that no shader reads: its name starts with
    /// `pad`.
    #[must_use]
    pub fn is_padding(&self) -> bool {
        self.name.starts_with("pad")
    }
}

/// A `bool` as a uniform block holds it: 4 bytes, 1 for `true` and 0 for
/// `false`. GLSL ES reads it as a `bool`; WGSL, as a `u32`, and MSL and the
/// Vulkan/D3D12 GLSL, as an `int`, which none can read a `bool` member as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(transparent)]
pub struct Bool32(u32);

impl From<bool> for Bool32 {
    fn from(value: bool) -> Bool32 {
        Bool32(u32::from(value))
    }
}

impl From<Bool32> for bool {
    fn from(value: Bool32) -> bool {
        value.0 != 0
    }
}

/// A Rust struct that a shader reads as a uniform block, or as an element
/// of a storage buffer: [`uniform_block!`](crate::uniform_block) declares
/// it.
pub trait UniformBlock: bytemuck::Pod {
    /// Its layout.
    const BLOCK: &'static Block;
}

/// The Rust type of a member of a [`Block`].
pub trait MemberValue: bytemuck::Pod {
    /// Its type in a block.
    const TYPE: MemberType;
}

impl MemberValue for f32 {
    const TYPE: MemberType = MemberType::Float;
}

impl MemberValue for i32 {
    const TYPE: MemberType = MemberType::Int;
}

impl MemberValue for u32 {
    const TYPE: MemberType = MemberType::Uint;
}

impl MemberValue for Bool32 {
    const TYPE: MemberType = MemberType::Bool;
}

impl MemberValue for [f32; 2] {
    const TYPE: MemberType = MemberType::Vec2;
}

impl MemberValue for [f32; 3] {
    const TYPE: MemberType = MemberType::Vec3;
}

impl MemberValue for [f32; 4] {
    const TYPE: MemberType = MemberType::Vec4;
}

impl MemberValue for [f32; 16] {
    const TYPE: MemberType = MemberType::Mat4;
}

impl<const N: usize> MemberValue for [[f32; 4]; N] {
    const TYPE: MemberType = MemberType::Vec4Array(N);
}

/// Declares a uniform block: a `#[repr(C, align(16))]` struct, `Pod` and
/// `Default`, whose members are its fields, and its [`UniformBlock`]
/// layout, which the backends declare in their shading languages.
///
/// A member's type is one of [`MemberValue`]'s, and the struct has no
/// padding but its members: a member that the shading languages align to
/// more than 4 bytes takes an offset that they align it to, which padding
/// members before it make up. A member's doc comments come before its
/// GLSL ES precision, `#[glsl_es(lowp)]` or `#[glsl_es(mediump)]`, which is
/// `highp` without one. The struct's doc comments come before
/// `#[glsl_es(prefix = "...")]`, which names its members in GLSL ES with
/// another prefix than `u_`.
///
/// ```
/// mln_gfx::uniform_block! {
///     /// A dot's color and radius (32 bytes).
///     pub struct DotPropsUBO {
///         pub color: [f32; 4],
///         #[glsl_es(mediump)]
///         pub radius: f32,
///         pub pad1: f32,
///         pub pad2: [f32; 2],
///     }
/// }
/// ```
#[macro_export]
macro_rules! uniform_block {
    (
        $(#[doc = $doc:literal])*
        $(#[glsl_es(prefix = $prefix:literal)])?
        $vis:vis struct $name:ident {
            $(
                $(#[doc = $member_doc:literal])*
                $(#[glsl_es($precision:ident)])?
                pub $member:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[doc = $doc])*
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        #[repr(C, align(16))]
        $vis struct $name {
            $($(#[doc = $member_doc])* pub $member: $ty,)*
        }

        const _: () = {
            const fn member<T: $crate::shaders::block::MemberValue>() {}
            $(member::<$ty>();)*
            assert!(
                0 $(+ ::core::mem::size_of::<$ty>())* == ::core::mem::size_of::<$name>(),
                "a uniform block has no padding but its members",
            );
        };

        // SAFETY: the struct is `repr(C)`, its fields are `Pod` and fill it
        // (asserted above), so every bit pattern is a value and it has no
        // uninitialized bytes.
        unsafe impl $crate::__private::bytemuck::Zeroable for $name {}
        // SAFETY: as for `Zeroable`.
        unsafe impl $crate::__private::bytemuck::Pod for $name {}

        impl $crate::shaders::block::UniformBlock for $name {
            const BLOCK: &'static $crate::shaders::block::Block = &$crate::shaders::block::Block {
                name: ::core::stringify!($name),
                size: ::core::mem::size_of::<$name>(),
                members: &[$($crate::shaders::block::Member {
                    name: ::core::stringify!($member),
                    ty: <$ty as $crate::shaders::block::MemberValue>::TYPE,
                    offset: ::core::mem::offset_of!($name, $member),
                    precision: $crate::uniform_block!(@precision $($precision)?),
                },)*],
                glsl_es_prefix: $crate::uniform_block!(@prefix $($prefix)?),
            };
        }
    };
    (@precision) => { $crate::shaders::block::Precision::High };
    (@precision mediump) => { $crate::shaders::block::Precision::Medium };
    (@precision lowp) => { $crate::shaders::block::Precision::Low };
    (@prefix) => { "u_" };
    (@prefix $prefix:literal) => { $prefix };
}

/// The members of `block` as `<type> <name>;` lines in a C-like shading
/// language, the types of `ty` and the arrays of `array`, and a
/// `<vec4 type> stride_pad[<n>];` line that pads the block to `size` bytes,
/// each line indented by four spaces.
pub(crate) fn c_like_members(
    block: &Block,
    size: usize,
    ty: impl Fn(MemberType) -> &'static str,
    vec4: &str,
) -> String {
    let mut members = String::new();
    for member in block.members {
        match member.ty {
            MemberType::Vec4Array(n) => _ = writeln!(members, "    {vec4} {}[{n}];", member.name),
            other => _ = writeln!(members, "    {} {};", ty(other), member.name),
        }
    }
    if size > block.size {
        _ = writeln!(members, "    {vec4} stride_pad[{}];", (size - block.size) / 16);
    }
    members
}

#[cfg(test)]
#[expect(unsafe_code, reason = "`uniform_block!` implements bytemuck's `Pod` for the block it declares")]
mod tests {
    use super::*;

    crate::uniform_block! {
        /// A test block.
        #[glsl_es(prefix = "u_test_")]
        pub(crate) struct TestUBO {
            pub matrix: [f32; 16],
            /// A color.
            #[glsl_es(lowp)]
            pub color: [f32; 3],
            pub flag: i32,
            pub lights: [[f32; 4]; 2],
        }
    }

    #[test]
    fn a_block_has_its_members_at_their_offsets() {
        let block = <TestUBO as UniformBlock>::BLOCK;
        assert_eq!(block.name, "TestUBO");
        assert_eq!(block.size, 112);
        assert_eq!(block.glsl_es_prefix, "u_test_");
        let members: Vec<_> = block.members.iter().map(|member| (member.name, member.ty, member.offset)).collect();
        assert_eq!(
            members,
            [
                ("matrix", MemberType::Mat4, 0),
                ("color", MemberType::Vec3, 64),
                ("flag", MemberType::Int, 76),
                ("lights", MemberType::Vec4Array(2), 80)
            ]
        );
        assert_eq!(block.members[1].precision, Precision::Low);
        assert_eq!(block.members[0].precision, Precision::High);
    }
}
