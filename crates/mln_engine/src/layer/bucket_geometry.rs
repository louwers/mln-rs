// Where a bucket's vertices and indices live once the render thread draws
// them.
//
// Buckets cross from the worker threads, so they hold plain vectors
// (`GeometryVec`). A bucket's upload moves each vector into the context's
// stores, which the draws of the bucket's tile share, so the bucket and the
// draws don't each keep a copy; the tile keeps their handles, by the key of
// the vector they were made of, in a `BucketGeometry`. The stores free the
// data once it is in a GPU buffer, and the draws a layer builds again for
// the same bucket reuse that buffer. Vertices that a layer makes of a
// bucket's vector are kept there too, made before the vector moves.
//
// A copy of a vector keeps its key. The tiles of a tile's other world
// copies hold copies of its buckets, and the upload of either takes what
// the other already moved into the stores, freeing its own copy.
//
// In the browser, the arrays a bucket gets from a tile worker in another
// wasm instance stay the JS arrays they arrived in (mln_wire::host_bytes)
// and move to the store as they are, which uploads them from there.
// Anything that reads or changes them first copies them into wasm memory.

use std::borrow::Cow;

use mln_util::identity::SimpleIdentity;
#[cfg(target_family = "wasm")]
use mln_wire::host_bytes::HostBytes;
use mln_wire::payload::{Payload, pod};

use crate::draw::context::Context;
use crate::draw::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::draw::vertices::SharedVertices;
use crate::layer::paint_property_binder::InterleavedVertexBuffer;

/// A bucket's vertices or indices. The worker fills it through `get_mut`;
/// the render thread can move the data out (`BucketGeometry`), after which
/// `len` still counts it.
#[derive(Clone, Debug)]
pub(crate) struct GeometryVec<T> {
    data: Data<T>,
    /// What `BucketGeometry` keeps the vector's stored geometry under. A
    /// copy keeps it, so the stored geometry of either serves both.
    key: SimpleIdentity,
}

/// Where a [`GeometryVec`]'s data is.
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

impl<T> Default for GeometryVec<T> {
    fn default() -> Self {
        GeometryVec::new(Vec::new())
    }
}

impl<T> GeometryVec<T> {
    #[must_use]
    pub(crate) fn new(v: Vec<T>) -> Self {
        GeometryVec::of(Data::Vec(v))
    }

    fn of(data: Data<T>) -> Self {
        GeometryVec { data, key: SimpleIdentity::new() }
    }

    /// The data, to fill. Not after it is moved out.
    pub(crate) fn get_mut(&mut self) -> &mut Vec<T>
    where
        T: bytemuck::Pod,
    {
        debug_assert!(!self.is_moved(), "geometry modified after it moved to the render thread");
        self.data.vec_mut()
    }

    /// The element count, which stays when the data is moved out.
    pub(crate) fn len(&self) -> usize {
        match &self.data {
            Data::Vec(v) => v.len(),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => bytes.len() / size_of::<T>(),
            Data::Moved { len } => *len,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once the data has moved to the context's store.
    pub(crate) fn is_moved(&self) -> bool {
        matches!(self.data, Data::Moved { .. })
    }

    /// The data, empty once it is moved out.
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

impl<T: bytemuck::Pod> GeometryVec<T> {
    /// Frees the data, which nothing will draw. `len` still counts it.
    pub(crate) fn discard(&mut self) {
        self.take();
    }

    /// The data, moved out into `context`'s store.
    fn store_vertices(&mut self, context: &mut Context) -> SharedVertices {
        match self.take() {
            Data::Vec(v) => context.add_vertices(v),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => context.add_host_vertices::<T>(bytes),
            Data::Moved { .. } => context.add_vertices(Vec::<T>::new()),
        }
    }
}

impl GeometryVec<u16> {
    /// Indexes of the data, moved out.
    fn indexes(&mut self) -> Indexes {
        match self.take() {
            Data::Vec(v) => Indexes::new(v),
            #[cfg(target_family = "wasm")]
            Data::Host(bytes) => Indexes::from_host(bytes),
            Data::Moved { .. } => Indexes::new(Vec::new()),
        }
    }
}

impl<T: bytemuck::Pod + PartialEq> PartialEq for GeometryVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.is_moved() == other.is_moved() && self.len() == other.len() && self.data() == other.data()
    }
}

impl<T> From<Vec<T>> for GeometryVec<T> {
    fn from(v: Vec<T>) -> Self {
        GeometryVec::new(v)
    }
}

/// As the plain values' bytes (`payload::pod`). Workers send buckets before
/// the render thread moves anything out. A payload the host kept in JS
/// stays there (`mln_wire::host_bytes`).
impl<T: bytemuck::Pod> mln_wire::Wire for GeometryVec<T> {
    fn write(&self, w: &mut mln_wire::Writer) {
        debug_assert!(!self.is_moved());
        pod::write(&self.data(), w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        Some(match pod::read_payload::<T>(r)? {
            Payload::Bytes(bytes) => GeometryVec::new(bytemuck::pod_collect_to_vec(&bytes)),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => GeometryVec::of(Data::Host(bytes)),
        })
    }
}

/// The geometry of a tile's buckets in the context's stores, by the key of
/// the bucket's vector it was made of. The tile keeps one per layout result,
/// so it lives as long as the buckets it was made from.
#[derive(Clone, Default)]
pub(crate) struct BucketGeometry {
    vertices: Kept<SharedVertices>,
    indexes: Kept<SharedIndexes>,
    /// The vertices a layer made of a bucket's vector.
    derived: Kept<SharedVertices>,
}

impl BucketGeometry {
    /// The vertices that `vec`, a bucket's, moved into.
    fn vertices<V>(&self, vec: &GeometryVec<V>) -> Option<&SharedVertices> {
        self.vertices.get(vec.key)
    }

    /// Frees the room that the entries' lists keep for more, where it is
    /// over a quarter of what they keep, once a tile's buckets uploaded.
    pub(crate) fn shrink(&mut self) {
        self.vertices.shrink();
        self.indexes.shrink();
        self.derived.shrink();
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

/// A bucket's upload: what moves the bucket's geometry into the context's
/// stores, and keeps its handles in its tile's [`BucketGeometry`].
pub(crate) struct GeometryUpload<'a> {
    geometry: &'a mut BucketGeometry,
    /// The geometry of the tile's other world copies, whose buckets the
    /// tile's are copies of, or they of the tile's.
    copies: &'a [&'a BucketGeometry],
    context: &'a mut Context,
}

impl<'a> GeometryUpload<'a> {
    /// The upload of buckets whose geometry `geometry` keeps, into
    /// `context`'s stores, which takes what of it `copies`, the geometry of
    /// the tile's other world copies, keep.
    pub(crate) fn new(
        geometry: &'a mut BucketGeometry,
        copies: &'a [&'a BucketGeometry],
        context: &'a mut Context,
    ) -> Self {
        GeometryUpload { geometry, copies, context }
    }

    /// Moves the data of `vec`, a bucket's, into the store, unless it moved
    /// before. Where a copy of it moved, this frees it and shares the
    /// copy's.
    pub(crate) fn move_vertices<V: bytemuck::Pod>(&mut self, vec: &mut GeometryVec<V>) {
        if vec.is_moved() {
            return;
        }
        if self.geometry.vertices.has_or_takes(vec.key, self.copies.iter().map(|copy| &copy.vertices)) {
            vec.discard();
        } else {
            let vertices = vec.store_vertices(self.context);
            self.geometry.vertices.insert(vec.key, vertices);
        }
    }

    /// Moves the data of `vec`, a bucket's, into the store's indexes,
    /// unless it moved before, as [`move_vertices`](Self::move_vertices)
    /// does.
    pub(crate) fn move_indexes(&mut self, vec: &mut GeometryVec<u16>) {
        if vec.is_moved() {
            return;
        }
        if self.geometry.indexes.has_or_takes(vec.key, self.copies.iter().map(|copy| &copy.indexes)) {
            vec.discard();
        } else {
            let indexes = self.context.add_indexes(vec.indexes());
            self.geometry.indexes.insert(vec.key, indexes);
        }
    }

    /// Moves the values of `buffer`, a layer's data-driven paint values,
    /// into the store, unless they `depend_on_feature_state`: those stay in
    /// the bucket, where feature state rewrites them.
    pub(crate) fn move_paint(&mut self, buffer: &mut InterleavedVertexBuffer, depends_on_feature_state: bool) {
        if !depends_on_feature_state {
            self.move_vertices(&mut buffer.shared_vertex_vector);
        }
    }

    /// Keeps the vertices that `make` makes of the data of `vec`, a
    /// bucket's, for [`derived_vertices`], unless it or a copy has some.
    /// Before the data moves.
    pub(crate) fn derive_vertices<V: bytemuck::Pod, D: bytemuck::Pod>(
        &mut self,
        vec: &GeometryVec<V>,
        make: impl FnOnce(&[V]) -> Vec<D>,
    ) {
        if !self.geometry.derived.has_or_takes(vec.key, self.copies.iter().map(|copy| &copy.derived)) {
            debug_assert!(!vec.is_moved(), "vertices derived from data that moved to the store");
            let derived = self.context.add_vertices(make(&vec.data()));
            self.geometry.derived.insert(vec.key, derived);
        }
    }
}

/// The vertices that a bucket's upload made of `vec`, one of its vectors,
/// with [`GeometryUpload::derive_vertices`], which its tile's `geometry`
/// keeps.
pub(crate) fn derived_vertices<V>(geometry: &BucketGeometry, vec: &GeometryVec<V>) -> Option<SharedVertices> {
    let derived = geometry.derived.get(vec.key);
    debug_assert!(derived.is_some(), "vertices that the bucket's upload didn't derive");
    derived.cloned()
}

/// Vertices of `context`'s store holding a copy of `vertices`, which the
/// draws that bind them don't change: draws replace such vertices instead.
pub(crate) fn copied_vertices<V: bytemuck::Pod>(context: &mut Context, vertices: &[V]) -> SharedVertices {
    context.add_vertices(vertices.to_vec())
}

/// Indexes holding a copy of `indexes`, which a draw owns.
#[must_use]
pub(crate) fn copied_indexes(indexes: &[u16]) -> DrawIndexes {
    DrawIndexes::new(indexes.to_vec())
}

/// The vertices that a bucket's upload moved `vec`, one of its vectors,
/// into, which its tile's `geometry` keeps.
pub(crate) fn bucket_vertices<V>(geometry: &BucketGeometry, vec: &GeometryVec<V>) -> Option<SharedVertices> {
    let vertices = geometry.vertices(vec);
    debug_assert!(vertices.is_some(), "vertices that the bucket's upload didn't move");
    vertices.cloned()
}

/// The indexes that a bucket's upload moved `vec`, one of its vectors,
/// into, which its tile's `geometry` keeps and its draws share.
pub(crate) fn bucket_indexes(geometry: &BucketGeometry, vec: &GeometryVec<u16>) -> Option<SharedIndexes> {
    let indexes = geometry.indexes.get(vec.key);
    debug_assert!(indexes.is_some(), "indexes that the bucket's upload didn't move");
    indexes.cloned()
}

/// The vertices of a layer's data-driven paint values: those that a
/// bucket's upload moved, which its tile's `geometry` keeps, or a copy in
/// `context`'s store of the values that feature state rewrites, which stay
/// in the bucket, and which the draws copy again each time they change.
pub(crate) fn paint_vertices(
    context: &mut Context,
    geometry: &BucketGeometry,
    buffer: &InterleavedVertexBuffer,
    depends_on_feature_state: bool,
) -> Option<SharedVertices> {
    if depends_on_feature_state {
        Some(copied_vertices(context, &buffer.shared_vertex_vector.data()))
    } else {
        bucket_vertices(geometry, &buffer.shared_vertex_vector)
    }
}
