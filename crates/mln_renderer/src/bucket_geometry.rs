// Port-only: where a bucket's vertices and indices live once the render
// thread draws them.
//
// C++ buckets keep their geometry in shared gfx vectors: the drawables point
// at them, an upload attaches the GPU buffers to them, and a released
// vector frees its CPU copy once it is in a buffer. The port's buckets cross
// from the worker threads, so they hold plain vectors (`GeometryVec`), and
// the render thread keeps the gfx vectors made from them, per tile, in a
// `BucketGeometry`. A bucket's upload moves each vector into its gfx vector,
// so the bucket and the draws don't each keep a copy. The gfx vector is
// released, so it frees the data once it is uploaded, and the draws a layer
// builds again for the same bucket reuse its buffer. Vertices that a layer
// makes of a bucket's vector are kept there too, made before the vector
// moves.
//
// In the browser, the arrays a bucket gets from a tile worker in another
// wasm instance stay the JS arrays they arrived in (mln_util::host_bytes)
// and move to the gfx vector as they are, which uploads them from there.
// Anything that reads or changes them first copies them into wasm memory.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
use mln_util::hash::HashMap;
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::identity::SimpleIdentity;
use mln_util::serialization::payload::{Payload, pod};

use crate::paint_property_binder::InterleavedVertexBuffer;

/// A bucket's vertices or indices. The worker fills it through `get_mut`;
/// the render thread can move the data out (`BucketGeometry`), after which
/// `len` still counts it.
pub(crate) struct GeometryVec<T> {
    v: Vec<T>,
    /// The data while it is still in the JS array it arrived in; `v` is
    /// empty then.
    #[cfg(target_family = "wasm")]
    host: Option<HostBytes>,
    /// What `BucketGeometry` keeps the vector's gfx vectors under, from the
    /// first time it keeps any. A copy keeps it, so a copy of the
    /// `BucketGeometry` serves the copy.
    key: Option<SimpleIdentity>,
    /// The element count of the data moved out.
    moved: usize,
}

impl<T> Default for GeometryVec<T> {
    fn default() -> Self {
        GeometryVec::new(Vec::new())
    }
}

impl<T> GeometryVec<T> {
    #[must_use]
    pub(crate) fn new(v: Vec<T>) -> Self {
        GeometryVec {
            v,
            #[cfg(target_family = "wasm")]
            host: None,
            key: None,
            moved: 0,
        }
    }

    /// The data, to fill. Not after it is moved out.
    pub(crate) fn get_mut(&mut self) -> &mut Vec<T>
    where
        T: bytemuck::Pod,
    {
        debug_assert!(!self.is_moved(), "geometry modified after it moved to the render thread");
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.host.take() {
            self.v = bytes.to_pod_vec();
        }
        &mut self.v
    }

    /// The element count, which stays when the data is moved out.
    pub(crate) fn len(&self) -> usize {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = &self.host {
            return bytes.len() / size_of::<T>() + self.moved;
        }
        self.v.len() + self.moved
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once the data has moved to the render thread's gfx vector.
    pub(crate) fn is_moved(&self) -> bool {
        self.moved != 0
    }

    /// The data, empty once it is moved out.
    pub(crate) fn data(&self) -> Cow<'_, [T]>
    where
        T: bytemuck::Pod,
    {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = &self.host {
            return Cow::Owned(bytes.to_pod_vec());
        }
        Cow::Borrowed(&self.v)
    }

    /// The key of the vector's gfx vectors, which this gives it the first
    /// time.
    fn key(&mut self) -> SimpleIdentity {
        *self.key.get_or_insert_with(SimpleIdentity::new)
    }
}

impl<T: bytemuck::Pod> GeometryVec<T> {
    /// The data, moved out.
    fn take(&mut self) -> Vec<T> {
        let data = std::mem::take(&mut self.v);
        self.moved += data.len();
        data
    }

    /// The data, moved out if it is still in the JS array it arrived in.
    #[cfg(target_family = "wasm")]
    fn take_host(&mut self) -> Option<HostBytes> {
        let bytes = self.host.take()?;
        self.moved += bytes.len() / size_of::<T>();
        Some(bytes)
    }

    /// Frees the data, which nothing will draw. `len` still counts it.
    pub(crate) fn discard(&mut self) {
        #[cfg(target_family = "wasm")]
        if self.take_host().is_some() {
            return;
        }
        drop(self.take());
    }

    /// A released gfx vector of the data, moved out.
    fn vertex_vector(&mut self) -> VertexVector<T> {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.take_host() {
            return VertexVector::from_host_released(bytes);
        }
        VertexVector::from_vec_released(self.take())
    }
}

impl GeometryVec<u16> {
    /// A released gfx index vector of the data, moved out.
    fn index_vector(&mut self) -> IndexVectorBase {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.take_host() {
            return IndexVectorBase::from_host_released(bytes);
        }
        IndexVectorBase::new_released(self.take())
    }
}

impl<T: Clone> Clone for GeometryVec<T> {
    fn clone(&self) -> Self {
        GeometryVec {
            v: self.v.clone(),
            #[cfg(target_family = "wasm")]
            host: self.host.clone(),
            key: self.key,
            moved: self.moved,
        }
    }
}

impl<T: bytemuck::Pod + PartialEq> PartialEq for GeometryVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.moved == other.moved && self.data() == other.data()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for GeometryVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("GeometryVec");
        f.field("v", &self.v);
        #[cfg(target_family = "wasm")]
        f.field("host", &self.host);
        f.field("key", &self.key).field("moved", &self.moved).finish()
    }
}

impl<T> From<Vec<T>> for GeometryVec<T> {
    fn from(v: Vec<T>) -> Self {
        GeometryVec::new(v)
    }
}

/// As the plain values' bytes (`payload::pod`). Workers send buckets before
/// the render thread moves anything out. A payload the host kept in JS
/// stays there (`mln_util::host_bytes`).
impl<T: bytemuck::Pod> mln_wire::Wire for GeometryVec<T> {
    fn write(&self, w: &mut mln_wire::Writer) {
        debug_assert!(!self.is_moved());
        pod::write(&self.data(), w);
    }

    fn read(r: &mut mln_wire::Reader<'_>) -> Option<Self> {
        Some(match pod::read_payload::<T>(r)? {
            Payload::Bytes(bytes) => GeometryVec::new(bytemuck::pod_collect_to_vec(&bytes)),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => GeometryVec { host: Some(bytes), ..GeometryVec::default() },
        })
    }
}

/// The gfx vectors of a tile's bucket geometry, by the key of the bucket's
/// vector they were made of. The tile keeps one per layout result, so it
/// lives as long as the buckets it was made from.
#[derive(Clone, Default)]
pub(crate) struct BucketGeometry {
    vertices: HashMap<SimpleIdentity, VertexVectorBasePtr>,
    indexes: HashMap<SimpleIdentity, IndexVectorBasePtr>,
    /// The vertices a layer made of a bucket's vector.
    derived: HashMap<SimpleIdentity, VertexVectorBasePtr>,
}

impl BucketGeometry {
    /// Moves the data of `vec`, a bucket's, into a shared vertex vector,
    /// unless it moved before.
    pub(crate) fn move_vertices<V: bytemuck::Pod>(&mut self, vec: &mut GeometryVec<V>) {
        let key = vec.key();
        self.vertices.entry(key).or_insert_with(|| Rc::new(vec.vertex_vector()));
    }

    /// Moves the data of `vec`, a bucket's, into a shared index vector,
    /// unless it moved before.
    pub(crate) fn move_indexes(&mut self, vec: &mut GeometryVec<u16>) {
        let key = vec.key();
        self.indexes.entry(key).or_insert_with(|| Rc::new(RefCell::new(vec.index_vector())));
    }

    /// Moves the values of `buffer`, a layer's data-driven paint values,
    /// into a shared vertex vector, unless they `depend_on_feature_state`:
    /// those stay in the bucket, where feature state rewrites them.
    pub(crate) fn move_paint(&mut self, buffer: &mut InterleavedVertexBuffer, depends_on_feature_state: bool) {
        if !depends_on_feature_state {
            self.move_vertices(&mut buffer.shared_vertex_vector);
        }
    }

    /// Keeps the vertices that `make` makes of the data of `vec`, a
    /// bucket's, for `derived_vertices`, unless it has some. Before the data
    /// moves.
    pub(crate) fn derive_vertices<V: bytemuck::Pod>(
        &mut self,
        vec: &mut GeometryVec<V>,
        make: impl FnOnce(&[V]) -> VertexVectorBasePtr,
    ) {
        let key = vec.key();
        self.derived.entry(key).or_insert_with(|| {
            debug_assert!(!vec.is_moved(), "vertices derived from data that moved to its gfx vector");
            make(&vec.data())
        });
    }

    fn get_vertices<V>(&self, vec: &GeometryVec<V>) -> Option<&VertexVectorBasePtr> {
        self.vertices.get(&vec.key?)
    }
}

/// The vertex vector that `make` makes of one of a bucket's vectors, kept
/// with the tile's `geometry` when it has one (see
/// `BucketGeometry::derive_vertices`), else made each time.
pub(crate) fn derived_vertices<V: bytemuck::Pod>(
    geometry: Option<&BucketGeometry>,
    vec: &GeometryVec<V>,
    make: impl FnOnce(&[V]) -> VertexVectorBasePtr,
) -> VertexVectorBasePtr {
    match geometry.zip(vec.key).and_then(|(geometry, key)| geometry.derived.get(&key)) {
        Some(derived) => Rc::clone(derived),
        None => make(&vec.data()),
    }
}

/// A released gfx vertex vector holding a copy of `vertices`: draws
/// replace such vectors instead of modifying them, so they free their CPU
/// copy once uploaded.
pub(crate) fn released_vertices<V: bytemuck::Pod>(vertices: &[V]) -> VertexVectorBasePtr {
    Rc::new(VertexVector::from_vec_released(vertices.to_vec()))
}

/// A released gfx index vector holding a copy of `indexes`, see
/// `released_vertices`.
#[must_use]
pub(crate) fn released_indexes(indexes: &[u16]) -> IndexVectorBasePtr {
    Rc::new(RefCell::new(IndexVectorBase::new_released(indexes.to_vec())))
}

/// The shared vertex vector of one of a bucket's vectors, from the tile's
/// `geometry` once the bucket's upload moved it there, else a copy.
pub(crate) fn bucket_vertices<V: bytemuck::Pod>(
    geometry: Option<&BucketGeometry>,
    vec: &GeometryVec<V>,
) -> VertexVectorBasePtr {
    match geometry.and_then(|geometry| geometry.get_vertices(vec)) {
        Some(vertices) => Rc::clone(vertices),
        None => released_vertices(&vec.data()),
    }
}

/// The shared index vector of one of a bucket's vectors, see
/// `bucket_vertices`.
pub(crate) fn bucket_indexes(geometry: Option<&BucketGeometry>, vec: &GeometryVec<u16>) -> IndexVectorBasePtr {
    match geometry.zip(vec.key).and_then(|(geometry, key)| geometry.indexes.get(&key)) {
        Some(indexes) => Rc::clone(indexes),
        None => released_indexes(&vec.data()),
    }
}

/// The shared vertex vector of a layer's data-driven paint values. Values
/// that feature state rewrites stay in the bucket, and the draws get a copy
/// each time they change.
pub(crate) fn paint_vertices(
    geometry: Option<&BucketGeometry>,
    buffer: &InterleavedVertexBuffer,
    depends_on_feature_state: bool,
) -> VertexVectorBasePtr {
    if depends_on_feature_state {
        released_vertices(&buffer.shared_vertex_vector.data())
    } else {
        bucket_vertices(geometry, &buffer.shared_vertex_vector)
    }
}
