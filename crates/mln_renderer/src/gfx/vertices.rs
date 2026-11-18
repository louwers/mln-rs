//! The vertices that draws bind from the context's store: a bucket's, which
//! a tile's draws share, and those a layer makes for its draws. Draws hold
//! them as [`SharedVertices`]; only the holder of a [`TypedVertices`], who
//! made them of a type, can replace them with new vertices of it. Only the
//! store makes either, of the entries it adds, so a handle's count and
//! vertex type are its entry's.

use std::marker::PhantomData;
use std::rc::Rc;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{BufferUsage, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;

use crate::gfx::context::Device;
use crate::gfx::store::{ReleasedEntries, Shared, Store, VerticesKey};

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
    /// The bytes the next upload writes, `Freed` once it wrote them.
    data: VertexData,
    count: u32,
    vertex_size: u32,
    buffer: Option<BufferPtr>,
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

    /// The bytes, while they are in wasm memory, to change.
    fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            VertexData::Bytes(bytes) => Some(bytes),
            VertexData::Halves(data) => Some(bytemuck::cast_slice_mut(data)),
            VertexData::Words(data) => Some(bytemuck::cast_slice_mut(data)),
            VertexData::Doubles(data) => Some(bytemuck::cast_slice_mut(data)),
            #[cfg(target_family = "wasm")]
            VertexData::Host(_) => None,
            VertexData::Freed => None,
        }
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
    fn new<V: bytemuck::Pod>(vertices: Vec<V>) -> Vertices {
        let count = vertices.len() as u32;
        let data = VertexData::of(vertices);
        Vertices { data, count, vertex_size: size_of::<V>() as u32, buffer: None }
    }

    /// The vertices of `vertex_size` bytes each in a JS array.
    #[cfg(target_family = "wasm")]
    fn from_host(bytes: HostBytes, vertex_size: usize) -> Vertices {
        debug_assert!(bytes.len().is_multiple_of(vertex_size.max(1)));
        let count = (bytes.len() / vertex_size.max(1)) as u32;
        Vertices { data: VertexData::Host(bytes), count, vertex_size: vertex_size as u32, buffer: None }
    }

    /// The number of vertices, which stays once the upload freed them.
    #[must_use]
    pub(crate) fn count(&self) -> usize {
        self.count as usize
    }

    /// The bytes a vertex takes.
    #[must_use]
    pub(crate) fn vertex_size(&self) -> usize {
        self.vertex_size as usize
    }

    /// The bytes the vertices take.
    fn size(&self) -> usize {
        self.count() * self.vertex_size()
    }

    /// Whether the next upload writes the vertices: they are new, or
    /// updated since the last upload, which wrote and freed their bytes.
    /// A draw bound before then binds them again, and its other attributes
    /// with them.
    #[must_use]
    pub(crate) fn needs_upload(&self) -> bool {
        !matches!(self.data, VertexData::Freed)
    }

    /// The bytes of the vertices while they are in wasm memory.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        self.data.bytes().unwrap_or_default()
    }

    /// Replaces the bytes with `bytes`, of as many vertices, which the next
    /// upload writes. Returns false, leaving the vertices be, for another
    /// number of bytes.
    fn update(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() != self.size() {
            return false;
        }
        match self.data.bytes_mut() {
            Some(data) => data.copy_from_slice(bytes),
            None => self.data = VertexData::Bytes(bytes.to_vec()),
        }
        true
    }

    /// The buffer of the vertices through `device`: made of them by the
    /// first upload, and written over by the upload after an update, each
    /// of which frees their bytes; `None` where there are none.
    fn upload(&mut self, device: &mut Device) -> Result<Option<BufferPtr>, GpuError> {
        if let Some(buffer) = &self.buffer {
            if let Some(bytes) = self.data.bytes() {
                device.write(buffer, bytes)?;
                self.data = VertexData::Freed;
            }
            return Ok(Some(BufferPtr::clone(buffer)));
        }
        let size = self.size();
        if size == 0 {
            self.data = VertexData::Freed;
            return Ok(None);
        }
        let buffer = match &self.data {
            #[cfg(target_family = "wasm")]
            VertexData::Host(bytes) => device.buffer_from_host(BufferUsage::Vertex, bytes)?,
            data => {
                let Some(bytes) = data.bytes() else {
                    debug_assert!(false, "vertices without data or a buffer");
                    return Err(GpuError::Invalid("vertices without data or a buffer".to_owned()));
                };
                device.buffer(BufferUsage::Vertex, &bytes[..size])?
            }
        };
        self.buffer = Some(BufferPtr::clone(&buffer));
        self.data = VertexData::Freed;
        Ok(Some(buffer))
    }
}

/// The context's store of the vertices that draws bind, which their
/// handles keep while they last.
#[derive(Default)]
pub(crate) struct VertexStore(Store<VerticesKey, Vertices>);

impl VertexStore {
    /// Keeps `vertices` while a clone of the returned handle lasts, whose
    /// key goes to `released` once the last clone drops.
    pub(crate) fn add<V: bytemuck::Pod>(
        &mut self,
        vertices: Vec<V>,
        released: &Rc<ReleasedEntries>,
    ) -> TypedVertices<V> {
        TypedVertices { shared: self.insert(Vertices::new(vertices), released), vertex_type: PhantomData }
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add`](Self::add) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host<V: bytemuck::Pod>(
        &mut self,
        bytes: HostBytes,
        released: &Rc<ReleasedEntries>,
    ) -> SharedVertices {
        self.insert(Vertices::from_host(bytes, size_of::<V>()), released)
    }

    fn insert(&mut self, vertices: Vertices, released: &Rc<ReleasedEntries>) -> SharedVertices {
        let count = vertices.count();
        SharedVertices { handle: self.0.add(vertices, released), count }
    }

    /// The vertices of `shared`.
    pub(crate) fn get(&self, shared: &SharedVertices) -> Option<&Vertices> {
        self.0.get(&shared.handle)
    }

    /// Whether the store has the vertices `key`.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: VerticesKey) -> bool {
        self.0.contains(key)
    }

    /// Replaces the vertices of `typed` with a copy of `data`, as many, as
    /// [`Vertices::update`] does.
    pub(crate) fn update<V: bytemuck::Pod>(&mut self, typed: &TypedVertices<V>, data: &[V]) -> bool {
        let Some(entry) = self.0.get_mut(&typed.shared.handle) else {
            return false;
        };
        debug_assert_eq!(entry.vertex_size(), size_of::<V>());
        entry.update(bytemuck::cast_slice(data))
    }

    /// The buffer of the vertices of `shared`, as [`Vertices::upload`]
    /// makes it.
    pub(crate) fn upload(
        &mut self,
        device: &mut Device,
        shared: &SharedVertices,
    ) -> Result<Option<BufferPtr>, GpuError> {
        self.0.get_mut(&shared.handle).ok_or(GpuError::UnknownId("vertices"))?.upload(device)
    }

    /// Drops the vertices `key`, whose last handle dropped.
    pub(crate) fn remove(&mut self, key: VerticesKey) {
        self.0.remove(key);
    }

    /// [`Store::compact`].
    pub(crate) fn compact(&mut self) {
        self.0.compact();
    }

    /// Drops every entry.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Vertices of the context's store that draws bind, with their count. They
/// are equal where they are the same entry.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SharedVertices {
    handle: Shared<VerticesKey>,
    count: usize,
}

impl SharedVertices {
    /// The entry's key in the store.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn key(&self) -> VerticesKey {
        self.handle.key()
    }

    /// The number of vertices.
    #[must_use]
    pub(crate) fn count(&self) -> usize {
        self.count
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
    /// The vertices, as draws bind them.
    #[must_use]
    pub(crate) fn shared(&self) -> &SharedVertices {
        &self.shared
    }

    /// [`shared`](Self::shared), without the type.
    #[must_use]
    pub(crate) fn into_shared(self) -> SharedVertices {
        self.shared
    }
}

impl<V> Clone for TypedVertices<V> {
    fn clone(&self) -> Self {
        TypedVertices { shared: self.shared.clone(), vertex_type: PhantomData }
    }
}
