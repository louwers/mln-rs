//! Uniform blocks, each declared once with `uniform_block!`: the Rust
//! struct the renderer writes, and the layout of its members that each
//! shading language reads it with.

/// The layout of a uniform block, or of an element of a storage buffer of
/// them, which `uniform_block!` declares with its Rust struct.
///
/// Each member is at its offset in the Rust struct in every layout that the
/// backends read a block with: `std140` uniform blocks and `std430` storage
/// buffers in GLSL and GLSL ES, WGSL's uniform and storage address spaces,
/// and MSL, which reads a [`Vec3`](MemberType::Vec3) as a `packed_float3`.
#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    /// The name of the Rust struct.
    pub name: &'static str,
    /// Its size in bytes, a multiple of 16.
    pub size: usize,
    /// Its members, in order, each after the one before it without a gap.
    pub members: &'static [Member],
}

/// A member of a [`Block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: &'static str,
    pub ty: MemberType,
    /// Its offset in bytes in the block.
    pub offset: usize,
}

/// The type of a [`Member`], by the Rust type of its field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberType {
    /// `f32`.
    Float,
    /// `i32`.
    Int,
    /// `u32`.
    Uint,
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

/// A Rust struct that a shader reads as a uniform block, or as an element
/// of a storage buffer: `uniform_block!` declares it.
pub trait UniformBlock: bytemuck::Pod {
    /// Its layout.
    const BLOCK: &'static Block;
}

/// The values of a draw's own of each of its program's blocks of each
/// draw, in the order the program lists them: none (`()`), one
/// [`UniformBlock`], or a pair. Only these implement it,
/// so that its bytes are always its blocks'.
pub trait DrawBlocks: Copy + sealed::Sealed + 'static {
    /// The layouts of the blocks, in order.
    const BLOCKS: &'static [&'static Block];

    /// Calls `f` with the bytes of each block, in order.
    fn each_bytes(&self, f: impl FnMut(&[u8]));
}

mod sealed {
    /// Seals [`DrawBlocks`](super::DrawBlocks) to the types this module
    /// implements it for.
    pub trait Sealed {}

    impl Sealed for () {}
    impl<T: super::UniformBlock> Sealed for T {}
    impl<A: super::UniformBlock, B: super::UniformBlock> Sealed for (A, B) {}
}

impl DrawBlocks for () {
    const BLOCKS: &'static [&'static Block] = &[];

    fn each_bytes(&self, _f: impl FnMut(&[u8])) {}
}

impl<T: UniformBlock> DrawBlocks for T {
    const BLOCKS: &'static [&'static Block] = &[T::BLOCK];

    fn each_bytes(&self, mut f: impl FnMut(&[u8])) {
        f(bytemuck::bytes_of(self));
    }
}

impl<A: UniformBlock, B: UniformBlock> DrawBlocks for (A, B) {
    const BLOCKS: &'static [&'static Block] = &[A::BLOCK, B::BLOCK];

    fn each_bytes(&self, mut f: impl FnMut(&[u8])) {
        f(bytemuck::bytes_of(&self.0));
        f(bytemuck::bytes_of(&self.1));
    }
}

/// The Rust type of a [`Member`] of a block.
pub trait MemberValue: bytemuck::Pod {
    /// Its type in a block.
    const TYPE: MemberType;
    /// The bytes its offset in a block is a multiple of in every layout
    /// [`Block`] names: 4 for a scalar, 8 for a `Vec2` and 16 for the others.
    const ALIGN: usize = match Self::TYPE {
        MemberType::Float | MemberType::Int | MemberType::Uint => 4,
        MemberType::Vec2 => 8,
        MemberType::Vec3 | MemberType::Vec4 | MemberType::Mat4 | MemberType::Vec4Array(_) => 16,
    };
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
/// `Default`, whose fields are its members, and its [`UniformBlock`]
/// layout.
///
/// A member's type is one of [`MemberValue`]'s. The build fails unless the
/// layouts that [`Block`] names all put each member where the Rust struct
/// has it: the struct has no padding, which bytemuck's `Pod` checks, so its
/// size is a multiple of 16 and each member follows the one before it, and
/// each member's offset is a multiple of its
/// [`ALIGN`](MemberValue::ALIGN). A block makes up a gap with members it
/// names `pad*`.
///
/// ```ignore
/// uniform_block! {
///     /// A dot's color and radius.
///     pub struct DotPropsUBO {
///         pub color: [f32; 4],
///         pub radius: f32,
///         pub pad1: f32,
///         pub pad2: [f32; 2],
///     }
/// }
/// ```
#[macro_export]
macro_rules! uniform_block {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$member_meta:meta])*
                pub $member:ident: $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
        #[repr(C, align(16))]
        $vis struct $name {
            $($(#[$member_meta])* pub $member: $ty,)*
        }

        const _: () = {
            $(assert!(
                ::core::mem::offset_of!($name, $member) % <$ty as $crate::shaders::block::MemberValue>::ALIGN == 0,
                ::core::concat!(
                    ::core::stringify!($name), "::", ::core::stringify!($member),
                    " is at an offset that the shading languages don't align it to",
                ),
            );)*
        };

        impl $crate::shaders::block::UniformBlock for $name {
            const BLOCK: &'static $crate::shaders::block::Block = &$crate::shaders::block::Block {
                name: ::core::stringify!($name),
                size: ::core::mem::size_of::<$name>(),
                members: &[$($crate::shaders::block::Member {
                    name: ::core::stringify!($member),
                    ty: <$ty as $crate::shaders::block::MemberValue>::TYPE,
                    offset: ::core::mem::offset_of!($name, $member),
                },)*],
            };
        }
    };
}

pub use crate::uniform_block;

#[cfg(test)]
mod tests {
    use super::*;

    uniform_block! {
        /// A test block.
        struct TestUBO {
            pub matrix: [f32; 16],
            /// A color.
            pub color: [f32; 3],
            pub flag: i32,
            pub lights: [[f32; 4]; 2],
            pub scale: [f32; 2],
            pub count: u32,
            pub pad1: f32,
        }
    }

    #[test]
    fn a_block_has_its_members_at_their_offsets() {
        let block = TestUBO::BLOCK;
        assert_eq!((block.name, block.size), ("TestUBO", 128));
        let members: Vec<_> = block.members.iter().map(|member| (member.name, member.ty, member.offset)).collect();
        assert_eq!(
            members,
            [
                ("matrix", MemberType::Mat4, 0),
                ("color", MemberType::Vec3, 64),
                ("flag", MemberType::Int, 76),
                ("lights", MemberType::Vec4Array(2), 80),
                ("scale", MemberType::Vec2, 112),
                ("count", MemberType::Uint, 120),
                ("pad1", MemberType::Float, 124),
            ]
        );
    }
}
