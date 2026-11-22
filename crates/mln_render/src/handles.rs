//! The handles of the renderer's resources that a layer keeps: buffers and
//! textures.

use std::marker::PhantomData;

use mln_gfx::gfx_types::AttributeDataType;

use crate::gfx::store::{SharedIndexes, SharedTexture};
use crate::gfx::vertices::SharedVertices;

/// A buffer of `V`s in the renderer's [`Resources`](crate::draw::Resources):
/// vertices that a mesh reads ([`MeshDesc::vertices`](crate::draw::MeshDesc::vertices)),
/// or, of `u16`s, the indexes of its primitives
/// ([`MeshDesc::indexes`](crate::draw::MeshDesc::indexes)).
pub struct Buffer<V> {
    pub(crate) entry: BufferEntry,
    len: usize,
    element: PhantomData<fn() -> V>,
}

/// Where a [`Buffer`]'s data is in the renderer's stores.
#[derive(Clone)]
pub(crate) enum BufferEntry {
    Vertices(SharedVertices),
    Indexes(SharedIndexes),
}

impl<V> Buffer<V> {
    pub(crate) fn new(entry: BufferEntry, len: usize) -> Buffer<V> {
        Buffer { entry, len, element: PhantomData }
    }

    /// The number of `V`s in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<V> Clone for Buffer<V> {
    fn clone(&self) -> Self {
        Buffer { entry: self.entry.clone(), len: self.len, element: PhantomData }
    }
}

impl<V> std::fmt::Debug for Buffer<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer").field("len", &self.len).finish_non_exhaustive()
    }
}

/// A type of vertex that a mesh reads: the formats of its attributes, in
/// the order that the program's attributes read them, each right after the
/// last in the vertex. A buffer of them binds to as many of the program's
/// attributes, from the first that no earlier buffer bound.
pub trait Vertex: bytemuck::Pod {
    /// The formats of the vertex's attributes, whose sizes add up to the
    /// vertex's.
    const ATTRIBUTES: &'static [AttributeDataType];
}

macro_rules! vertices {
    ($($ty:ty => $format:ident,)*) => {$(
        impl Vertex for $ty {
            const ATTRIBUTES: &'static [AttributeDataType] = &[AttributeDataType::$format];
        }
    )*};
}

vertices! {
    [i16; 2] => Short2,
    [i16; 4] => Short4,
    [u16; 2] => UShort2,
    [u16; 4] => UShort4,
    [u8; 4] => UByte4,
    f32 => Float,
    [f32; 2] => Float2,
    [f32; 3] => Float3,
    [f32; 4] => Float4,
}

/// A texture in the renderer's [`Resources`](crate::draw::Resources), which
/// a mesh samples ([`MeshDesc::texture`](crate::draw::MeshDesc::texture)),
/// with the sampler it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture(pub(crate) SharedTexture);
