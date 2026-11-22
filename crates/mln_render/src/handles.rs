//! The handles of the renderer's resources that a layer keeps: buffers and
//! textures.

use std::marker::PhantomData;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::segment::SegmentBase;

use crate::gfx::store::SharedTexture;
use crate::gfx::vertices::SharedVertices;

/// A buffer of `V`s in the renderer's [`Resources`](crate::draw::Resources):
/// vertices that a mesh reads ([`MeshDesc::vertices`](crate::draw::MeshDesc::vertices)),
/// or, of `u16`s, the indexes of its primitives
/// ([`MeshDesc::indexes`](crate::draw::MeshDesc::indexes)).
pub struct Buffer<V> {
    pub(crate) entry: SharedVertices,
    element: PhantomData<fn() -> V>,
}

impl<V> Buffer<V> {
    pub(crate) fn new(entry: SharedVertices) -> Buffer<V> {
        Buffer { entry, element: PhantomData }
    }

    /// The number of `V`s in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entry.count()
    }

    /// Whether the buffer holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<V> Clone for Buffer<V> {
    fn clone(&self) -> Self {
        Buffer { entry: self.entry.clone(), element: PhantomData }
    }
}

impl<V> std::fmt::Debug for Buffer<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer").field("len", &self.len()).finish_non_exhaustive()
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

/// A buffer of `u16` indexes in the renderer's
/// [`Resources`](crate::draw::Resources), each relative to the first vertex
/// of its segment, with the segments that draw them: a mesh's primitives
/// ([`MeshDesc::indexes`](crate::draw::MeshDesc::indexes)).
#[derive(Clone, Debug)]
pub struct IndexBuffer {
    pub(crate) buffer: Buffer<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

/// A texture in the renderer's [`Resources`](crate::draw::Resources), which
/// a mesh samples ([`MeshDesc::texture`](crate::draw::MeshDesc::texture)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture(pub(crate) SharedTexture);

/// The surface that a tile's geometry covers in a frame's projection, in
/// the tile's units: its extent's corners on Mercator, and on the globe a
/// grid that bends with the planet and reaches a pole where the tile
/// touches one, with the triangles that cover it
/// ([`Resources::tile_surface`](crate::draw::Resources::tile_surface)).
#[derive(Clone, Debug)]
pub struct TileSurface {
    /// The vertices.
    pub vertices: Buffer<[i16; 2]>,
    /// Their triangles.
    pub triangles: IndexBuffer,
}
