// Where a bucket's vertices and indices live once the render thread draws
// them.
//
// Buckets cross from the worker threads, so they hold plain vectors
// (`GpuVec`). A bucket's upload moves each vector into a buffer of the
// resources, which the meshes of the bucket's tile share, so the bucket and
// the meshes don't each keep a copy; the tile keeps the buffers, by the key
// of the vector they were made of, in a `BucketGeometry`. The stores free
// the data once it is in a GPU buffer.
//
// A copy of a vector keeps its key. The tiles of a tile's other world
// copies hold copies of its buckets, and the upload of either takes what
// the other already moved into the stores, freeing its own copy.
//
// In the browser, the arrays a bucket gets from a tile worker in another
// wasm instance stay the JS arrays they arrived in (mln_util::host_bytes)
// and move to the store as they are, which uploads them from there.
// Anything that reads or changes them first copies them into wasm memory.

use std::any::Any;
use std::borrow::Cow;
use std::rc::Rc;

#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::identity::SimpleIdentity;
use mln_util::serialization::payload::{Payload, pod};

use crate::gfx::buffer_store::SharedBuffer;
use crate::gfx::context::Resources;
use crate::gfx::mesh::Indexes;
use crate::handles::{Buffer, IndexBuffer};
use crate::paint_property_binder::InterleavedVertexBuffer;

/// A bucket's vertices or indexes, which the engine uploads into a
/// [`Buffer`](crate::draw::Buffer) on the renderer's thread
/// ([`Upload::vec`](crate::layer::Upload::vec)). The layout fills it through
/// [`get_mut`](Self::get_mut); once it uploaded, its data is gone, and
/// [`len`](Self::len) still counts it.
#[derive(Clone, Debug)]
pub struct GpuVec<T> {
    data: Data<T>,
    /// What `BucketGeometry` keeps the vector's stored geometry under. A
    /// copy keeps it, so the stored geometry of either serves both.
    key: SimpleIdentity,
}

/// Where a [`GpuVec`]'s data is.
#[derive(Clone, Debug)]
enum Data<T> {
    /// In memory.
    Vec(Vec<T>),
    /// In the JS array it arrived in.
    #[cfg(target_family = "wasm")]
    Host(HostBytes),
    /// Moved out, `len` elements of it.
    Moved { len: usize },
}

impl<T> Default for GpuVec<T> {
    fn default() -> Self {
        GpuVec::new(Vec::new())
    }
}

impl<T> GpuVec<T> {
    #[must_use]
    pub(crate) fn new(v: Vec<T>) -> Self {
        GpuVec::of(Data::Vec(v))
    }

    fn of(data: Data<T>) -> Self {
        GpuVec { data, key: SimpleIdentity::new() }
    }

    /// The data, which is gone once it uploaded unless the bucket keeps it
    /// ([`Upload::kept`](crate::layer::Upload::kept)).
    #[must_use]
    pub fn get(&self) -> &[T] {
        match &self.data {
            Data::Vec(v) => v,
            #[cfg(target_family = "wasm")]
            Data::Host(_) => {
                debug_assert!(false, "the data of a vector still in the JS array it arrived in");
                &[]
            }
            Data::Moved { .. } => &[],
        }
    }

    /// The data, to fill, or to change where the bucket keeps it. Not after
    /// it uploaded otherwise.
    pub fn get_mut(&mut self) -> &mut Vec<T>
    where
        T: bytemuck::Pod,
    {
        debug_assert!(!self.is_moved(), "geometry modified after it moved to the render thread");
        self.data.vec_mut()
    }

    /// The element count, which stays when the data is moved out.
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.data {
            Data::Vec(v) => v.len(),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => bytes.len() / size_of::<T>(),
            Data::Moved { len } => *len,
        }
    }

    /// Whether the vector holds no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once the data has moved to the context's store.
    #[must_use]
    pub(crate) fn is_moved(&self) -> bool {
        matches!(self.data, Data::Moved { .. })
    }

    /// The data, empty once it is moved out.
    #[must_use]
    pub(crate) fn data(&self) -> Cow<'_, [T]>
    where
        T: bytemuck::Pod,
    {
        self.data.data()
    }

    /// The data, moved out, leaving its count.
    fn take(&mut self) -> Data<T> {
        let len = self.len();
        std::mem::replace(&mut self.data, Data::Moved { len })
    }
}

impl<T: bytemuck::Pod> Data<T> {
    /// The data, empty once it is moved out.
    fn data(&self) -> Cow<'_, [T]> {
        match self {
            Data::Vec(v) => Cow::Borrowed(v),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => Cow::Owned(bytes.to_pod_vec()),
            Data::Moved { .. } => Cow::Borrowed(&[]),
        }
    }

    /// The data's vector, which replaces the data where it isn't in memory:
    /// a copy of the JS array's, or an empty one where it moved out.
    fn vec_mut(&mut self) -> &mut Vec<T> {
        match self {
            Data::Vec(v) => v,
            #[cfg(target_family = "wasm")]
            host @ Data::Host(_) => {
                *host = Data::Vec(host.data().into_owned());
                host.vec_mut()
            }
            moved @ Data::Moved { .. } => {
                *moved = Data::Vec(Vec::new());
                moved.vec_mut()
            }
        }
    }
}

impl<T: bytemuck::Pod> GpuVec<T> {
    /// Frees the data, which nothing will draw. `len` still counts it.
    pub(crate) fn discard(&mut self) {
        self.take();
    }

    /// The data, moved out into `context`'s store.
    fn store_buffer(&mut self, context: &mut Resources) -> SharedBuffer {
        match self.take() {
            Data::Vec(v) => context.add_buffer(v),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => context.add_host_buffer::<T>(bytes),
            Data::Moved { .. } => context.add_buffer(Vec::<T>::new()),
        }
    }
}

impl<T: bytemuck::Pod + PartialEq> PartialEq for GpuVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.is_moved() == other.is_moved() && self.len() == other.len() && self.data() == other.data()
    }
}

impl<T> From<Vec<T>> for GpuVec<T> {
    fn from(v: Vec<T>) -> Self {
        GpuVec::new(v)
    }
}

/// As the plain values' bytes (`payload::pod`). Workers send buckets before
/// the render thread moves anything out. A payload the host kept in JS
/// stays there (`mln_util::host_bytes`).
impl<T: bytemuck::Pod> mln_wire::Wire for GpuVec<T> {
    fn write(&self, w: &mut mln_wire::Writer) {
        debug_assert!(!self.is_moved());
        pod::write(&self.data(), w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        Some(match pod::read_payload::<T>(r)? {
            Payload::Bytes(bytes) => GpuVec::new(bytemuck::pod_collect_to_vec(&bytes)),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => GpuVec::of(Data::Host(bytes)),
        })
    }
}

/// The buffers of a tile's buckets, by the key of the bucket's vector they
/// were made of. The tile keeps one per layout result, so it lives as long
/// as the buckets it was made from.
#[derive(Clone, Default)]
pub(crate) struct BucketGeometry {
    /// Each a [`Buffer`] of the vector's element type.
    buffers: Kept<Rc<dyn Any>>,
}

impl BucketGeometry {
    /// The buffer that `vec`, a bucket's, uploaded into.
    pub(crate) fn buffer<V: 'static>(&self, vec: &GpuVec<V>) -> Option<&Buffer<V>> {
        self.buffers.get(vec.key)?.downcast_ref()
    }

    /// The buffer of the paint values of `vertices`, a bucket's; `None` for
    /// values that no vertex reads.
    pub(crate) fn paint(&self, vertices: &InterleavedVertexBuffer) -> Option<&Buffer<u8>> {
        self.buffer(vertices.vertices())
    }

    /// Uploads `vec`, a bucket's, into a buffer of `res`, freeing its data
    /// unless `keep`. Where it uploaded before, it writes kept data over the
    /// buffer's; where a copy of a vector it frees did, in `copies`, the
    /// geometry of the tile's other world copies, it shares that buffer. A
    /// kept vector's data may differ by world copy, so each has its own.
    pub(crate) fn upload<V: bytemuck::Pod>(
        &mut self,
        vec: &mut GpuVec<V>,
        keep: bool,
        copies: &[&BucketGeometry],
        res: &mut Resources,
    ) {
        if vec.is_moved() {
            return;
        }
        let copies = if keep { &[] } else { copies };
        if self.buffers.has_or_takes(vec.key, copies.iter().map(|copy| &copy.buffers)) {
            if !keep {
                vec.discard();
            } else if let Some(buffer) = self.buffer(vec) {
                let rewrote = res.rewrite(&buffer.entry, bytemuck::cast_slice(vec.get()));
                debug_assert!(rewrote, "a kept vector whose length changed");
            }
            return;
        }
        let entry = if keep { res.add_buffer(vec.get_mut().clone()) } else { vec.store_buffer(res) };
        let buffer: Rc<dyn Any> = Rc::new(Buffer::<V>::new(entry));
        self.buffers.insert(vec.key, buffer);
    }

    /// The index buffer that `indexes`, a bucket's, uploaded into.
    pub(crate) fn indexes(&self, indexes: &Indexes) -> Option<&IndexBuffer> {
        self.buffers.get(indexes.indexes.key)?.downcast_ref()
    }

    /// Uploads `indexes`, a bucket's, into an index buffer of `res`, freeing
    /// their data unless `keep`, as [`upload`](Self::upload) does a vector.
    pub(crate) fn upload_indexes(
        &mut self,
        indexes: &mut Indexes,
        keep: bool,
        copies: &[&BucketGeometry],
        res: &mut Resources,
    ) {
        let vec = &mut indexes.indexes;
        if vec.is_moved() {
            return;
        }
        let copies = if keep { &[] } else { copies };
        if self.buffers.has_or_takes(vec.key, copies.iter().map(|copy| &copy.buffers)) {
            if !keep {
                vec.discard();
            } else if let Some(uploaded) = self.indexes(indexes) {
                let rewrote = res.rewrite(&uploaded.buffer.entry, bytemuck::cast_slice(indexes.indexes.get()));
                debug_assert!(rewrote, "kept indexes whose length changed");
            }
            return;
        }
        let entry = if keep { res.add_buffer(vec.get_mut().clone()) } else { vec.store_buffer(res) };
        let uploaded: Rc<dyn Any> =
            Rc::new(IndexBuffer { buffer: Buffer::new(entry), segments: indexes.segments.clone() });
        self.buffers.insert(vec.key, uploaded);
    }

    /// Frees the room that the entries' lists keep for more, where it is
    /// over a quarter of what they keep, once a tile's buckets uploaded.
    pub(crate) fn shrink(&mut self) {
        self.buffers.shrink();
    }
}

/// Entries by the key of the vector they were made of, in key order: a
/// tile's are few, and a list takes less memory than a map.
#[derive(Clone)]
struct Kept<T>(Vec<(SimpleIdentity, T)>);

impl<T> Default for Kept<T> {
    fn default() -> Self {
        Kept(Vec::new())
    }
}

impl<T> Kept<T> {
    fn get(&self, key: SimpleIdentity) -> Option<&T> {
        let at = self.0.binary_search_by_key(&key, |(key, _)| *key).ok()?;
        self.0.get(at).map(|(_, value)| value)
    }

    fn insert(&mut self, key: SimpleIdentity, value: T) {
        match self.0.binary_search_by_key(&key, |(key, _)| *key) {
            Ok(at) => {
                if let Some(entry) = self.0.get_mut(at) {
                    entry.1 = value;
                }
            }
            Err(at) => self.0.insert(at, (key, value)),
        }
    }

    /// Whether the list has an entry under `key`, taking the first of
    /// `others`' where it has none.
    fn has_or_takes<'a>(&mut self, key: SimpleIdentity, mut others: impl Iterator<Item = &'a Kept<T>>) -> bool
    where
        T: Clone + 'a,
    {
        if self.get(key).is_some() {
            return true;
        }
        let Some(shared) = others.find_map(|other| other.get(key)) else {
            return false;
        };
        self.insert(key, shared.clone());
        true
    }

    fn shrink(&mut self) {
        if self.0.capacity() - self.0.len() > self.0.len() / 4 {
            self.0.shrink_to_fit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upload_moves_a_vectors_data_into_a_buffer_and_keeps_its_count() {
        let mut res = Resources::detached();
        let (mut vec, mut indexes) = (GpuVec::new(vec![[1i16, 2], [3, 4]]), Indexes::default());
        indexes.add(&mut vec![0u8; 3], 0, [[0, 1, 2]]);
        let mut geometry = BucketGeometry::default();
        geometry.upload(&mut vec, false, &[], &mut res);
        geometry.upload_indexes(&mut indexes, false, &[], &mut res);
        assert!(vec.get().is_empty() && indexes.get().is_empty(), "the data moved to the buffers");
        assert_eq!((vec.len(), geometry.buffer(&vec).map(Buffer::len)), (2, Some(2)));
        assert_eq!(geometry.indexes(&indexes).map(|uploaded| uploaded.buffer.len()), Some(3));
    }

    // The buckets of a tile's world copy are copies of the tile's, and the
    // upload of either shares what the other moved into the buffers, freeing
    // its own data.
    #[test]
    fn a_world_copys_upload_shares_the_buffers_of_its_copy() {
        let mut res = Resources::detached();
        let mut vec = GpuVec::new(vec![1.0f32, 2.0]);
        let mut copy = vec.clone();
        let mut copy_geometry = BucketGeometry::default();
        copy_geometry.upload(&mut copy, false, &[], &mut res);
        let mut geometry = BucketGeometry::default();
        geometry.upload(&mut vec, false, &[&copy_geometry], &mut res);
        assert!(vec.get().is_empty());
        assert!(geometry.buffer(&vec).is_some_and(|buffer| Some(buffer) == copy_geometry.buffer(&copy)));
    }

    // Data that changes in place, as feature state and placement change it,
    // stays, and each world copy has a buffer of its own of it.
    #[test]
    fn a_kept_vector_keeps_its_data_and_a_buffer_of_each_copys_own() {
        let mut res = Resources::detached();
        let mut vec = GpuVec::new(vec![1.0f32, 2.0]);
        let mut copy = vec.clone();
        let mut copy_geometry = BucketGeometry::default();
        copy_geometry.upload(&mut copy, true, &[], &mut res);
        let mut geometry = BucketGeometry::default();
        geometry.upload(&mut vec, true, &[&copy_geometry], &mut res);
        assert_eq!(vec.get(), [1.0, 2.0]);
        assert!(geometry.buffer(&vec).is_some_and(|buffer| Some(buffer) != copy_geometry.buffer(&copy)));
    }
}
