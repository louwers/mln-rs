//! The buffers that draws read from the context's store: a bucket's
//! vertices and indexes, which a tile's draws share, and those a layer
//! makes for its draws. Draws hold them as [`SharedVertices`]. Only the
//! store makes one, of the entries it adds, so a handle's count is its
//! entry's. An entry is a vertex or an index buffer as its first upload
//! says: a GPU buffer is one or the other. New bytes of the same size
//! replace an entry's, which the next upload writes over its buffer's.

use std::rc::Rc;

use mln_gfx::gpu::{BufferId, BufferUsage, Gpu, GpuError};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;

use crate::gfx::buffer::Buffer;
use crate::gfx::owned::Held;
use crate::gfx::release::ReleaseQueue;
use crate::gfx::resources::Device;
use crate::gfx::store::{Shared, Store, VerticesKey};

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
/// to a buffer, which frees them, and that buffer. An update gives them new
/// bytes of as many vertices, which the next upload writes over the
/// buffer's.
pub(crate) struct Vertices {
    /// The bytes the next upload writes; `Freed` once it wrote them.
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
        let count = vertices.len();
        let data = VertexData::of(vertices);
        Vertices { data, count, vertex_size: size_of::<V>(), buffer: None }
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
        !matches!(self.data, VertexData::Freed)
    }

    /// The GPU buffer, once an upload made it.
    #[must_use]
    pub(crate) fn buffer(&self) -> Option<BufferId> {
        self.buffer.as_ref().map(Buffer::id)
    }

    /// The bytes of the vertices while they are in wasm memory.
    #[must_use]
    pub(crate) fn bytes(&self) -> &[u8] {
        self.data.bytes().unwrap_or_default()
    }

    /// Replaces the bytes of the vertices with `bytes`, of as many
    /// vertices, which the next upload writes. Returns false, leaving the
    /// vertices be, for another number of bytes.
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

    /// Frees the bytes, which the upload wrote.
    fn written(&mut self) {
        self.data = VertexData::Freed;
    }

    /// The buffer of the vertices of `gpu` through `device`, of `usage`,
    /// which every upload of them names: made of them by the first upload,
    /// and written over by the upload after an update, each of which frees
    /// their bytes; `None` where there are none.
    fn upload(
        &mut self,
        gpu: &mut dyn Gpu,
        device: &mut Device,
        usage: BufferUsage,
    ) -> Result<Option<BufferId>, GpuError> {
        if let Some(buffer) = &self.buffer {
            if buffer.usage() != usage {
                debug_assert!(false, "a buffer read as {usage:?} after it was uploaded as {:?}", buffer.usage());
                return Err(GpuError::Invalid(format!("a {:?} buffer read as {usage:?}", buffer.usage())));
            }
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
            VertexData::Host(bytes) => device.buffer_from_host(gpu, usage, bytes)?,
            data => {
                let Some(bytes) = data.bytes() else {
                    debug_assert!(false, "vertices without data or a buffer");
                    return Err(GpuError::Invalid("vertices without data or a buffer".to_owned()));
                };
                device.buffer(gpu, usage, &bytes[..size])?
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

    /// Replaces the bytes of `shared` with `bytes`, as [`Vertices::update`]
    /// does.
    pub(crate) fn rewrite(&mut self, shared: &SharedVertices, bytes: &[u8]) -> bool {
        self.0.get_mut(&shared.handle).is_some_and(|entry| entry.update(bytes))
    }

    /// The buffer of `shared`, of `usage`, as [`Vertices::upload`] makes
    /// it.
    pub(crate) fn upload(
        &mut self,
        gpu: &mut dyn Gpu,
        device: &mut Device,
        shared: &SharedVertices,
        usage: BufferUsage,
    ) -> Result<Option<BufferId>, GpuError> {
        self.0.get_mut(&shared.handle).ok_or(GpuError::UnknownId("vertices"))?.upload(gpu, device, usage)
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
pub struct SharedVertices {
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
    pub fn count(&self) -> usize {
        self.count
    }
}
