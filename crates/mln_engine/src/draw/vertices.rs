//! The vertices that draws bind from the context's store: a bucket's, which
//! a tile's draws share, and those a layer makes for its draws. Draws hold
//! them as [`SharedVertices`]; only the holder of a [`TypedVertices`], who
//! made them as dynamic vertices of a type, can replace them with new
//! vertices of it. Only the store makes either, of the entries it adds, so a
//! handle's count and vertex type are its entry's.

use std::marker::PhantomData;
use std::rc::Rc;

use mln_gfx::gpu::{BufferId, BufferUsage, Gpu, GpuError};
#[cfg(target_family = "wasm")]
use mln_wire::host_bytes::HostBytes;

use crate::draw::buffer::Buffer;
use crate::draw::owned::Held;
use crate::draw::release::ReleaseQueue;
use crate::draw::resources::Device;
use crate::draw::store::{Shared, Store, VerticesKey};

/// Where the vertices' bytes are: in the vector of the alignment of their
/// type, which a vector of vertices moves into without a copy, or of
/// dynamic vertices in bytes.
enum VertexData {
    Bytes(Vec<u8>),
    Halves(Vec<u16>),
    Words(Vec<u32>),
    Doubles(Vec<u64>),
    /// Dynamic vertices' bytes, which the next upload writes.
    Dynamic(Vec<u8>),
    /// Dynamic vertices' bytes that the last upload wrote, whose storage the
    /// next update fills.
    Written(Vec<u8>),
    /// The bytes in the JS array they arrived in, which the upload writes
    /// from there.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
    /// Freed once in a buffer.
    Freed,
}

/// Vertices of the context's store: their bytes until an upload writes them
/// to a buffer, which frees them, and that buffer. Dynamic vertices take
/// updates, each new bytes of as many vertices, which the next upload
/// writes over the buffer's; their uploads keep their bytes' storage for
/// the next update to fill.
pub(crate) struct Vertices {
    /// The bytes the next upload writes; once it wrote them, `Freed`, or
    /// `Written` for dynamic vertices.
    data: VertexData,
    count: usize,
    vertex_size: usize,
    buffer: Option<Buffer>,
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
            VertexData::Dynamic(bytes) => Some(bytes),
            #[cfg(target_family = "wasm")]
            VertexData::Host(_) => None,
            VertexData::Written(_) | VertexData::Freed => None,
        }
    }

    /// The bytes, while they are in wasm memory.
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            VertexData::Bytes(bytes) => Some(bytes),
            VertexData::Halves(data) => Some(bytemuck::cast_slice(data)),
            VertexData::Words(data) => Some(bytemuck::cast_slice(data)),
            VertexData::Doubles(data) => Some(bytemuck::cast_slice(data)),
            VertexData::Dynamic(bytes) => Some(bytes),
            #[cfg(target_family = "wasm")]
            VertexData::Host(_) => None,
            VertexData::Written(_) | VertexData::Freed => None,
        }
    }
}

impl Vertices {
    /// `vertices`, moved.
    fn new<V: bytemuck::Pod>(vertices: Vec<V>) -> Vertices {
        let count = vertices.len();
        let data = VertexData::of(vertices);
        Vertices { data, count, vertex_size: size_of::<V>(), buffer: None }
    }

    /// Dynamic vertices of a copy of `vertices`.
    fn dynamic<V: bytemuck::Pod>(vertices: &[V]) -> Vertices {
        let data = VertexData::Dynamic(bytemuck::cast_slice(vertices).to_vec());
        Vertices { data, count: vertices.len(), vertex_size: size_of::<V>(), buffer: None }
    }

    /// The vertices of `vertex_size` bytes each in a JS array.
    #[cfg(target_family = "wasm")]
    fn from_host(bytes: HostBytes, vertex_size: usize) -> Vertices {
        debug_assert!(bytes.len().is_multiple_of(vertex_size.max(1)));
        let count = bytes.len() / vertex_size.max(1);
        Vertices { data: VertexData::Host(bytes), count, vertex_size, buffer: None }
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

    /// The bytes the vertices take.
    fn size(&self) -> usize {
        self.count() * self.vertex_size()
    }

    /// Whether the next upload writes the vertices: they are new, or
    /// updated since the last upload, which wrote their bytes. A draw bound
    /// before then binds them again, and its other attributes with them.
    #[must_use]
    pub(crate) fn needs_upload(&self) -> bool {
        !matches!(self.data, VertexData::Written(_) | VertexData::Freed)
    }

    /// The bytes of the vertices while they are in wasm memory.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        self.data.bytes().unwrap_or_default()
    }

    /// Replaces the bytes of dynamic vertices with `bytes`, of as many
    /// vertices, which the next upload writes. Returns false, leaving the
    /// vertices be, for another number of bytes.
    fn update(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() != self.size() {
            return false;
        }
        if let VertexData::Written(storage) = &mut self.data {
            self.data = VertexData::Dynamic(std::mem::take(storage));
        }
        match self.data.bytes_mut() {
            Some(data) => data.copy_from_slice(bytes),
            None => self.data = VertexData::Bytes(bytes.to_vec()),
        }
        true
    }

    /// Frees the bytes, which the upload wrote, or keeps those of dynamic
    /// vertices for the next update.
    fn written(&mut self) {
        self.data = match std::mem::replace(&mut self.data, VertexData::Freed) {
            VertexData::Dynamic(bytes) | VertexData::Written(bytes) => VertexData::Written(bytes),
            _ => VertexData::Freed,
        };
    }

    /// The buffer of the vertices of `gpu` through `device`: made of them
    /// by the first upload, and written over by the upload after an update,
    /// each of which frees their bytes, or keeps dynamic vertices' for the
    /// next update; `None` where there are none.
    fn upload(&mut self, gpu: &mut dyn Gpu, device: &mut Device) -> Result<Option<BufferId>, GpuError> {
        if let Some(buffer) = &self.buffer {
            let id = buffer.id();
            if let Some(bytes) = self.data.bytes() {
                device.write(gpu, buffer, bytes)?;
                self.written();
            }
            return Ok(Some(id));
        }
        let size = self.size();
        if size == 0 {
            self.written();
            return Ok(None);
        }
        let buffer = match &self.data {
            #[cfg(target_family = "wasm")]
            VertexData::Host(bytes) => device.buffer_from_host(gpu, BufferUsage::Vertex, bytes)?,
            data => {
                let Some(bytes) = data.bytes() else {
                    debug_assert!(false, "vertices without data or a buffer");
                    return Err(GpuError::Invalid("vertices without data or a buffer".to_owned()));
                };
                device.buffer(gpu, BufferUsage::Vertex, &bytes[..size])?
            }
        };
        let id = buffer.id();
        self.buffer = Some(buffer);
        self.written();
        Ok(Some(id))
    }
}

/// The context's store of the vertices that draws bind, which their
/// handles keep while they last.
#[derive(Default)]
pub(crate) struct VertexStore(Store<VerticesKey, Vertices>);

impl VertexStore {
    /// Keeps `vertices` while a clone of the returned handle lasts, whose
    /// key goes to `queue` once the last clone drops.
    pub(crate) fn add<V: bytemuck::Pod>(&mut self, vertices: Vec<V>, queue: &Rc<ReleaseQueue>) -> SharedVertices {
        self.insert(Vertices::new(vertices), queue)
    }

    /// Keeps a copy of `vertices` as dynamic vertices, which the holder of
    /// the returned handle updates ([`update`](Self::update)), as
    /// [`add`](Self::add) does.
    pub(crate) fn add_dynamic<V: bytemuck::Pod>(
        &mut self,
        vertices: &[V],
        queue: &Rc<ReleaseQueue>,
    ) -> TypedVertices<V> {
        TypedVertices { shared: self.insert(Vertices::dynamic(vertices), queue), vertex_type: PhantomData }
    }

    /// Keeps the vertices of the type `V` in the JS array `bytes`, as
    /// [`add`](Self::add) does.
    #[cfg(target_family = "wasm")]
    pub(crate) fn add_host<V: bytemuck::Pod>(&mut self, bytes: HostBytes, queue: &Rc<ReleaseQueue>) -> SharedVertices {
        self.insert(Vertices::from_host(bytes, size_of::<V>()), queue)
    }

    fn insert(&mut self, vertices: Vertices, queue: &Rc<ReleaseQueue>) -> SharedVertices {
        let count = vertices.count();
        SharedVertices { handle: self.0.add(vertices, queue), count }
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
        gpu: &mut dyn Gpu,
        device: &mut Device,
        shared: &SharedVertices,
    ) -> Result<Option<BufferId>, GpuError> {
        self.0.get_mut(&shared.handle).ok_or(GpuError::UnknownId("vertices"))?.upload(gpu, device)
    }

    /// Takes out the vertices `key`, whose last handle dropped.
    pub(crate) fn remove(&mut self, key: VerticesKey) -> Option<Vertices> {
        self.0.remove(key)
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
    /// What geometry made of the vertices holds.
    pub(crate) fn held(&self) -> Held {
        Held::Vertices(self.handle.clone())
    }

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

/// Dynamic vertices of the context's store made of vertices of the type
/// `V`, which their holder can replace with others of `V`
/// ([`Context::update_vertices`]).
///
/// [`Context::update_vertices`]: crate::draw::context::Context::update_vertices
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
}

impl<V> Clone for TypedVertices<V> {
    fn clone(&self) -> Self {
        TypedVertices { shared: self.shared.clone(), vertex_type: PhantomData }
    }
}
