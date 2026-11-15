// Port-only: where a bucket's vertices and indices live once the render
// thread draws them.
//
// C++ buckets keep their geometry in shared gfx vectors: the drawables point
// at them, an upload attaches the GPU buffers to them, and a released
// vector frees its CPU copy once it is in a buffer. The port's buckets are
// shared with the worker threads, so they hold plain vectors (`GeometryVec`),
// and the render thread keeps the gfx vectors made from them, per tile, in a
// `BucketGeometry`. A vector moves into its gfx vector the first time a layer
// draws it, so the bucket and the drawables don't each keep a copy. The gfx
// vector is released, so it frees the data once it is uploaded, and the
// drawables a layer builds again for the same bucket reuse its buffer.
// Vertices that a layer makes of a bucket's vector are kept there too, made
// before the vector moves.
//
// In the browser, the arrays a bucket gets from a tile worker in another
// wasm instance stay the JS arrays they arrived in (mln_util::host_bytes)
// and move to the gfx vector as they are, which uploads them from there.
// Anything that reads or changes them first copies them into wasm memory.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
use mln_util::hash::HashMap;
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::identity::SimpleIdentity;
use mln_util::serialization::payload::{Payload, pod};
use parking_lot::{Mutex, MutexGuard};

use crate::bucket::Bucket;

/// A bucket's vertices or indices. The worker fills it through `get_mut`;
/// the render thread can move the data out (`BucketGeometry`), after which
/// `len` still counts it.
pub(crate) struct GeometryVec<T> {
    v: Mutex<Vec<T>>,
    /// The data while it is still in the JS array it arrived in; `v` is
    /// empty then.
    #[cfg(target_family = "wasm")]
    host: Mutex<Option<HostBytes>>,
    /// The element count of the data moved out.
    moved: AtomicUsize,
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
            v: Mutex::new(v),
            #[cfg(target_family = "wasm")]
            host: Mutex::new(None),
            moved: AtomicUsize::new(0),
        }
    }

    /// The data, to fill. Not after it is moved out.
    pub(crate) fn get_mut(&mut self) -> &mut Vec<T>
    where
        T: bytemuck::Pod,
    {
        debug_assert!(!self.is_moved(), "geometry modified after it moved to the render thread");
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.host.get_mut().take() {
            *self.v.get_mut() = bytes.to_pod_vec();
        }
        self.v.get_mut()
    }

    /// The element count, which stays when the data is moved out.
    pub(crate) fn len(&self) -> usize {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = &*self.host.lock() {
            return bytes.len() / size_of::<T>() + self.moved.load(Ordering::Relaxed);
        }
        self.v.lock().len() + self.moved.load(Ordering::Relaxed)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once the data has moved to the render thread's gfx vector.
    pub(crate) fn is_moved(&self) -> bool {
        self.moved.load(Ordering::Relaxed) != 0
    }

    /// The data, empty once it is moved out.
    pub(crate) fn data(&self) -> MutexGuard<'_, Vec<T>>
    where
        T: bytemuck::Pod,
    {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.host.lock().take() {
            *self.v.lock() = bytes.to_pod_vec();
        }
        self.v.lock()
    }
}

impl<T: bytemuck::Pod> GeometryVec<T> {
    /// The data, moved out unless `keep` (which copies it).
    fn take(&self, keep: bool) -> Vec<T> {
        let mut v = self.data();
        if keep {
            return v.clone();
        }
        let data = std::mem::take(&mut *v);
        self.moved.fetch_add(data.len(), Ordering::Relaxed);
        data
    }

    /// The data, moved out if it is still in the JS array it arrived in.
    #[cfg(target_family = "wasm")]
    fn take_host(&self) -> Option<HostBytes> {
        let bytes = self.host.lock().take()?;
        self.moved.fetch_add(bytes.len() / size_of::<T>(), Ordering::Relaxed);
        Some(bytes)
    }

    /// Frees the data, which nothing will draw. `len` still counts it.
    pub(crate) fn discard(&self) {
        #[cfg(target_family = "wasm")]
        if self.take_host().is_some() {
            return;
        }
        drop(self.take(false));
    }

    /// A released gfx vector of the data, moved out unless `keep`.
    fn vertex_vector(&self, keep: bool) -> VertexVector<T> {
        #[cfg(target_family = "wasm")]
        if !keep && let Some(bytes) = self.take_host() {
            return VertexVector::from_host_released(bytes);
        }
        VertexVector::from_vec_released(self.take(keep))
    }
}

impl GeometryVec<u16> {
    /// A released gfx index vector of the data, moved out unless `keep`.
    fn index_vector(&self, keep: bool) -> IndexVectorBase {
        #[cfg(target_family = "wasm")]
        if !keep && let Some(bytes) = self.take_host() {
            return IndexVectorBase::from_host_released(bytes);
        }
        IndexVectorBase::new_released(self.take(keep))
    }
}

impl<T: Clone> Clone for GeometryVec<T> {
    fn clone(&self) -> Self {
        GeometryVec {
            v: Mutex::new(self.v.lock().clone()),
            #[cfg(target_family = "wasm")]
            host: Mutex::new(self.host.lock().clone()),
            moved: AtomicUsize::new(self.moved.load(Ordering::Relaxed)),
        }
    }
}

impl<T: bytemuck::Pod + PartialEq> PartialEq for GeometryVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.moved.load(Ordering::Relaxed) == other.moved.load(Ordering::Relaxed) && *self.data() == *other.data()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for GeometryVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("GeometryVec");
        f.field("v", &*self.v.lock());
        #[cfg(target_family = "wasm")]
        f.field("host", &*self.host.lock());
        f.field("moved", &self.moved.load(Ordering::Relaxed)).finish()
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
            Payload::Host(bytes) => {
                let vec = GeometryVec::default();
                *vec.host.lock() = Some(bytes);
                vec
            }
        })
    }
}

/// The gfx vectors of a tile's bucket geometry. The tile keeps one per
/// layout result, so it lives as long as the buckets it was made from.
#[derive(Default)]
pub(crate) struct BucketGeometry {
    vertices: RefCell<HashMap<(SimpleIdentity, usize), VertexVectorBasePtr>>,
    indexes: RefCell<HashMap<(SimpleIdentity, usize), IndexVectorBasePtr>>,
    /// The vertices a layer made of a bucket's vector, by the vector's key.
    derived: RefCell<HashMap<(SimpleIdentity, usize), VertexVectorBasePtr>>,
}

/// The key of a bucket's `GeometryVec`: the bucket and the vector's address,
/// which is fixed while the bucket lives.
fn key<T>(bucket: &dyn Bucket, vec: &GeometryVec<T>) -> (SimpleIdentity, usize) {
    (bucket.get_id(), std::ptr::from_ref(vec) as usize)
}

/// Nothing is moved out when the map reads the vertices back to report
/// rendered features.
fn keep(bucket: &dyn Bucket) -> bool {
    bucket.base().retain_features_by_id
}

/// The vector of `key` in `map`, which `make` makes the first time.
fn cached<T: ?Sized>(
    map: &RefCell<HashMap<(SimpleIdentity, usize), Rc<T>>>,
    key: (SimpleIdentity, usize),
    make: impl FnOnce() -> Rc<T>,
) -> Rc<T> {
    Rc::clone(map.borrow_mut().entry(key).or_insert_with(make))
}

impl BucketGeometry {
    /// The shared vertex vector of `vec`, one of `bucket`'s.
    pub(crate) fn vertices<V: bytemuck::Pod>(&self, bucket: &dyn Bucket, vec: &GeometryVec<V>) -> VertexVectorBasePtr {
        cached(&self.vertices, key(bucket, vec), || Rc::new(vec.vertex_vector(keep(bucket))))
    }

    /// The shared index vector of `vec`, one of `bucket`'s.
    pub(crate) fn indexes(&self, bucket: &dyn Bucket, vec: &GeometryVec<u16>) -> IndexVectorBasePtr {
        cached(&self.indexes, key(bucket, vec), || Rc::new(RefCell::new(vec.index_vector(keep(bucket)))))
    }

    /// The vertex vector that `make` makes of the data of `vec`, one of
    /// `bucket`'s, the first time it is asked for, which is before
    /// `vertices` moves the data. One per vector.
    pub(crate) fn derived_vertices<V: bytemuck::Pod>(
        &self,
        bucket: &dyn Bucket,
        vec: &GeometryVec<V>,
        make: impl FnOnce(&[V]) -> VertexVectorBasePtr,
    ) -> VertexVectorBasePtr {
        cached(&self.derived, key(bucket, vec), || {
            debug_assert!(!vec.is_moved(), "vertices derived from data that moved to its gfx vector");
            make(&vec.data())
        })
    }
}

/// The vertex vector that `make` makes of one of `bucket`'s vectors, kept
/// with the tile's `geometry` when it has one (see
/// `BucketGeometry::derived_vertices`), else made each time.
pub(crate) fn derived_vertices<V: bytemuck::Pod>(
    geometry: Option<&BucketGeometry>,
    bucket: &dyn Bucket,
    vec: &GeometryVec<V>,
    make: impl FnOnce(&[V]) -> VertexVectorBasePtr,
) -> VertexVectorBasePtr {
    match geometry {
        Some(geometry) => geometry.derived_vertices(bucket, vec, make),
        None => make(&vec.data()),
    }
}

/// A released gfx vertex vector holding a copy of `vertices`: drawables
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

/// The shared vertex vector of one of `bucket`'s vectors, from the tile's
/// `geometry` when it has one, else a copy.
pub(crate) fn bucket_vertices<V: bytemuck::Pod>(
    geometry: Option<&BucketGeometry>,
    bucket: &dyn Bucket,
    vec: &GeometryVec<V>,
) -> VertexVectorBasePtr {
    match geometry {
        Some(geometry) => geometry.vertices(bucket, vec),
        None => released_vertices(&vec.data()),
    }
}

/// The shared index vector of one of `bucket`'s vectors, see
/// `bucket_vertices`.
pub(crate) fn bucket_indexes(
    geometry: Option<&BucketGeometry>,
    bucket: &dyn Bucket,
    vec: &GeometryVec<u16>,
) -> IndexVectorBasePtr {
    match geometry {
        Some(geometry) => geometry.indexes(bucket, vec),
        None => released_indexes(&vec.data()),
    }
}

/// The shared vertex vector of a layer's data-driven paint values. Values
/// that feature state rewrites stay in the bucket, and the drawables get a
/// copy each time they change.
pub(crate) fn paint_vertices(
    geometry: Option<&BucketGeometry>,
    bucket: &dyn Bucket,
    buffer: &crate::paint_property_binder::InterleavedVertexBuffer,
    depends_on_feature_state: bool,
) -> VertexVectorBasePtr {
    if depends_on_feature_state {
        released_vertices(&buffer.shared_vertex_vector.data())
    } else {
        bucket_vertices(geometry, bucket, &buffer.shared_vertex_vector)
    }
}
