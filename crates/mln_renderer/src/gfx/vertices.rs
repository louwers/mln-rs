//! The vertices that draws bind from the context's store: a bucket's, which
//! a tile's draws share, and those a layer makes for its draws. Draws hold
//! them as [`SharedVertices`]; only the holder of a [`TypedVertices`], who
//! made them of a type, can replace them with new vertices of it.

use std::marker::PhantomData;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{BufferUsage, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::time::Instant;

use crate::gfx::context::Device;
use crate::gfx::store::{Shared, VerticesKey};

/// Where the vertices' bytes are: in the vector of the alignment of their
/// type, which a vector of vertices moves into without a copy.
enum VertexData {
    Bytes(Vec<u8>),
    Halves(Vec<u16>),
    Words(Vec<u32>),
    Doubles(Vec<u64>),
    /// The bytes in the JS array they arrived in, which the upload writes
    /// from there.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
    /// Freed once in a buffer.
    Freed,
}

/// Vertices of the context's store: their bytes until an upload writes them
/// to a buffer, which frees them, and that buffer. An update brings new
/// bytes of as many vertices, which the next upload writes over the
/// buffer's.
pub(crate) struct Vertices {
    data: VertexData,
    count: usize,
    vertex_size: usize,
    /// When the vertices were made or last updated.
    modified: Instant,
    /// The buffer, and the modification time of the vertices it holds.
    buffer: Option<(BufferPtr, Instant)>,
}

impl VertexData {
    /// The data of `vertices`, moved.
    fn of<V: bytemuck::Pod>(vertices: Vec<V>) -> VertexData {
        use bytemuck::allocation::try_cast_vec;
        let moved = match align_of::<V>() {
            1 => try_cast_vec(vertices).map(VertexData::Bytes),
            2 => try_cast_vec(vertices).map(VertexData::Halves),
            4 => try_cast_vec(vertices).map(VertexData::Words),
            8 => try_cast_vec(vertices).map(VertexData::Doubles),
            _ => Err((bytemuck::PodCastError::AlignmentMismatch, vertices)),
        };
        moved.unwrap_or_else(|(_, vertices)| VertexData::Bytes(bytemuck::cast_slice(&vertices).to_vec()))
    }

    /// The bytes, while they are in wasm memory.
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            VertexData::Bytes(bytes) => Some(bytes),
            VertexData::Halves(data) => Some(bytemuck::cast_slice(data)),
            VertexData::Words(data) => Some(bytemuck::cast_slice(data)),
            VertexData::Doubles(data) => Some(bytemuck::cast_slice(data)),
            #[cfg(target_family = "wasm")]
            VertexData::Host(_) => None,
            VertexData::Freed => None,
        }
    }
}

impl Vertices {
    /// `vertices`, moved.
    pub(crate) fn new<V: bytemuck::Pod>(vertices: Vec<V>) -> Vertices {
        let count = vertices.len();
        let data = VertexData::of(vertices);
        Vertices { data, count, vertex_size: size_of::<V>(), modified: Instant::now(), buffer: None }
    }

    /// The vertices of `vertex_size` bytes each in a JS array.
    #[cfg(target_family = "wasm")]
    pub(crate) fn from_host(bytes: HostBytes, vertex_size: usize) -> Vertices {
        debug_assert!(bytes.len().is_multiple_of(vertex_size.max(1)));
        let count = bytes.len() / vertex_size.max(1);
        Vertices { data: VertexData::Host(bytes), count, vertex_size, modified: Instant::now(), buffer: None }
    }

    /// The number of vertices, which stays once the upload freed them.
    #[must_use]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// The bytes a vertex takes.
    #[must_use]
    pub(crate) fn vertex_size(&self) -> usize {
        self.vertex_size
    }

    /// Whether the vertices' bytes are still here: false once an upload
    /// wrote them to their buffer, until an update brings new ones.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        !matches!(self.data, VertexData::Freed)
    }

    /// The bytes of the vertices while they are in wasm memory.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        self.data.bytes().unwrap_or_default()
    }

    /// Whether the vertices were made or updated after `time`. Still true
    /// once their upload freed them: a draw bound before then binds the
    /// buffer that holds them.
    #[must_use]
    pub(crate) fn is_modified_after(&self, time: Instant) -> bool {
        time < self.modified
    }

    /// Replaces the bytes with `bytes`, of as many vertices, which the next
    /// upload writes. Returns false, leaving the vertices be, for another
    /// number of bytes.
    fn update(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() != self.count * self.vertex_size {
            return false;
        }
        self.data = VertexData::Bytes(bytes.to_vec());
        self.modified = Instant::now();
        true
    }

    /// The buffer of the vertices through `device`, `None` where there are
    /// none. Where they changed since their buffer was written, or with
    /// `force`, it is written again, or replaced where they don't fit in it.
    /// The bytes go once in a buffer.
    pub(crate) fn upload(&mut self, device: &mut Device, force: bool) -> Result<Option<BufferPtr>, GpuError> {
        let size = self.count * self.vertex_size;
        if let Some((buffer, holds)) = &self.buffer
            && size <= buffer.size()
        {
            if let Some(bytes) = self.data.bytes()
                && (force || *holds < self.modified)
            {
                device.write(buffer, &bytes[..size])?;
                let buffer = BufferPtr::clone(buffer);
                return Ok(Some(self.hold(buffer)));
            }
            return Ok(Some(BufferPtr::clone(buffer)));
        }
        if size == 0 {
            return Ok(None);
        }
        let buffer = match (&self.data, self.data.bytes()) {
            (_, Some(bytes)) => device.buffer(BufferUsage::Vertex, &bytes[..size])?,
            #[cfg(target_family = "wasm")]
            (VertexData::Host(bytes), None) => device.buffer_from_host(BufferUsage::Vertex, bytes)?,
            _ => {
                debug_assert!(false, "vertices without data or a buffer they fit in");
                return Err(GpuError::Invalid("vertices without data or a buffer they fit in".to_owned()));
            }
        };
        Ok(Some(self.hold(buffer)))
    }

    /// Makes `buffer`, which holds the vertices, theirs, and frees their
    /// bytes.
    fn hold(&mut self, buffer: BufferPtr) -> BufferPtr {
        self.buffer = Some((BufferPtr::clone(&buffer), self.modified));
        self.data = VertexData::Freed;
        buffer
    }
}

/// Vertices of the context's store that draws bind, with their count.
#[derive(Clone)]
pub(crate) struct SharedVertices {
    handle: Shared<VerticesKey>,
    count: usize,
}

impl SharedVertices {
    pub(crate) fn new(handle: Shared<VerticesKey>, count: usize) -> SharedVertices {
        SharedVertices { handle, count }
    }

    /// The entry's handle.
    #[must_use]
    pub(crate) fn handle(&self) -> &Shared<VerticesKey> {
        &self.handle
    }

    /// The number of vertices.
    #[must_use]
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Whether `self` and `other` are the same vertices.
    #[must_use]
    pub(crate) fn same(&self, other: &SharedVertices) -> bool {
        self.handle.same(&other.handle)
    }
}

/// Vertices of the context's store made of vertices of the type `V`, which
/// their holder can replace with others of `V`
/// ([`Context::update_vertices`](crate::gfx::context::Context::update_vertices)).
pub(crate) struct TypedVertices<V> {
    shared: SharedVertices,
    vertex_type: PhantomData<fn() -> V>,
}

impl<V> TypedVertices<V> {
    pub(crate) fn new(shared: SharedVertices) -> TypedVertices<V> {
        TypedVertices { shared, vertex_type: PhantomData }
    }

    /// The vertices, as draws bind them.
    #[must_use]
    pub(crate) fn shared(&self) -> &SharedVertices {
        &self.shared
    }

    /// The vertices, as draws bind them.
    #[must_use]
    pub(crate) fn into_shared(self) -> SharedVertices {
        self.shared
    }
}

impl<V> Clone for TypedVertices<V> {
    fn clone(&self) -> Self {
        TypedVertices::new(self.shared.clone())
    }
}

/// Replaces the bytes of `vertices` with those of `data`.
pub(crate) fn update<V: bytemuck::Pod>(vertices: &mut Vertices, data: &[V]) -> bool {
    debug_assert_eq!(vertices.vertex_size, size_of::<V>());
    vertices.update(bytemuck::cast_slice(data))
}
