// Port-only: where a bucket's vertices and indices live once the render
// thread draws them.
//
// C++ buckets keep their geometry in shared gfx vectors: the drawables point
// at them, the upload pass attaches the GPU buffers to them, and a released
// vector frees its CPU copy once it is in a buffer. The port's buckets are
// shared with the worker threads, so they hold plain vectors (`GeometryVec`),
// and the render thread keeps the gfx vectors made from them, per tile, in a
// `BucketGeometry`. A vector moves into its gfx vector the first time a layer
// draws it, so the bucket and the drawables don't each keep a copy. The gfx
// vector is released, so it frees the data once it is uploaded, and the
// drawables a layer builds again for the same bucket reuse its buffer.
//
// In the browser, the arrays a bucket gets from a tile worker in another
// wasm instance stay the JS arrays they arrived in (mln_util::host_bytes)
// and move to the gfx vector as they are, which uploads them from there.
// Anything that reads or changes them first copies them into wasm memory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::vertex_vector::{VertexVector, VertexVectorBasePtr};
#[cfg(target_family = "wasm")]
use mln_util::host_bytes::HostBytes;
use mln_util::identity::SimpleIdentity;
use mln_util::serialization::payload::{Payload, pod};
use mln_util::sync::MutexExt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bucket::Bucket;

/// A bucket's vertices or indices. The worker fills it through `get_mut`;
/// the render thread can move the data out (`BucketGeometry`), after which
/// `len` still counts it.
pub struct GeometryVec<T> {
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
    pub fn new(v: Vec<T>) -> Self {
        GeometryVec {
            v: Mutex::new(v),
            #[cfg(target_family = "wasm")]
            host: Mutex::new(None),
            moved: AtomicUsize::new(0),
        }
    }

    /// The data, to fill. Not after it is moved out.
    pub fn get_mut(&mut self) -> &mut Vec<T>
    where
        T: bytemuck::Pod,
    {
        debug_assert!(!self.is_moved(), "geometry modified after it moved to the render thread");
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.host.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            *self.v.get_mut().unwrap_or_else(|e| e.into_inner()) = bytes.to_pod_vec();
        }
        self.v.get_mut().unwrap_or_else(|e| e.into_inner())
    }

    /// The element count, which stays when the data is moved out.
    pub fn len(&self) -> usize {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = &*self.host.lock_unpoisoned() {
            return bytes.len() / size_of::<T>() + self.moved.load(Ordering::Relaxed);
        }
        self.v.lock_unpoisoned().len() + self.moved.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True once the data has moved to the render thread's gfx vector.
    pub fn is_moved(&self) -> bool {
        self.moved.load(Ordering::Relaxed) != 0
    }

    /// The data, empty once it is moved out.
    pub fn data(&self) -> MutexGuard<'_, Vec<T>>
    where
        T: bytemuck::Pod,
    {
        #[cfg(target_family = "wasm")]
        if let Some(bytes) = self.host.lock_unpoisoned().take() {
            *self.v.lock_unpoisoned() = bytes.to_pod_vec();
        }
        self.v.lock_unpoisoned()
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
        let bytes = self.host.lock_unpoisoned().take()?;
        self.moved.fetch_add(bytes.len() / size_of::<T>(), Ordering::Relaxed);
        Some(bytes)
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
            v: Mutex::new(self.v.lock_unpoisoned().clone()),
            #[cfg(target_family = "wasm")]
            host: Mutex::new(self.host.lock_unpoisoned().clone()),
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
        f.field("v", &*self.v.lock_unpoisoned());
        #[cfg(target_family = "wasm")]
        f.field("host", &*self.host.lock_unpoisoned());
        f.field("moved", &self.moved.load(Ordering::Relaxed)).finish()
    }
}

impl<T> From<Vec<T>> for GeometryVec<T> {
    fn from(v: Vec<T>) -> Self {
        GeometryVec::new(v)
    }
}

/// As the plain values' bytes (`payload::pod`). Workers send buckets before
/// the render thread moves anything out.
impl<T: bytemuck::Pod> Serialize for GeometryVec<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        debug_assert!(!self.is_moved());
        pod::serialize(&self.data(), serializer)
    }
}

/// A payload the host kept in JS stays there (mln_util::host_bytes).
impl<'de, T: bytemuck::Pod> Deserialize<'de> for GeometryVec<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match pod::deserialize_payload::<T, D>(deserializer)? {
            Payload::Bytes(bytes) => GeometryVec::new(bytemuck::pod_collect_to_vec(&bytes)),
            #[cfg(target_family = "wasm")]
            Payload::Host(bytes) => {
                let vec = GeometryVec::default();
                *vec.host.lock_unpoisoned() = Some(bytes);
                vec
            }
        })
    }
}

/// The gfx vectors of a tile's bucket geometry. The tile keeps one per
/// layout result, so it lives as long as the buckets it was made from.
#[derive(Default)]
pub struct BucketGeometry {
    vertices: RefCell<HashMap<(SimpleIdentity, usize), VertexVectorBasePtr>>,
    indexes: RefCell<HashMap<(SimpleIdentity, usize), IndexVectorBasePtr>>,
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

impl BucketGeometry {
    /// The shared vertex vector of `vec`, one of `bucket`'s.
    pub fn vertices<V: bytemuck::Pod>(&self, bucket: &dyn Bucket, vec: &GeometryVec<V>) -> VertexVectorBasePtr {
        self.vertices
            .borrow_mut()
            .entry(key(bucket, vec))
            .or_insert_with(|| Rc::new(vec.vertex_vector(keep(bucket))))
            .clone()
    }

    /// The shared index vector of `vec`, one of `bucket`'s.
    pub fn indexes(&self, bucket: &dyn Bucket, vec: &GeometryVec<u16>) -> IndexVectorBasePtr {
        self.indexes
            .borrow_mut()
            .entry(key(bucket, vec))
            .or_insert_with(|| Rc::new(RefCell::new(vec.index_vector(keep(bucket)))))
            .clone()
    }
}

/// A released gfx vertex vector holding a copy of `vertices`: drawables
/// replace such vectors instead of modifying them, so they free their CPU
/// copy once uploaded.
pub fn released_vertices<V: bytemuck::Pod>(vertices: &[V]) -> VertexVectorBasePtr {
    Rc::new(VertexVector::from_vec_released(vertices.to_vec()))
}

/// A released gfx index vector holding a copy of `indexes`, see
/// `released_vertices`.
pub fn released_indexes(indexes: &[u16]) -> IndexVectorBasePtr {
    Rc::new(RefCell::new(IndexVectorBase::new_released(indexes.to_vec())))
}

/// The shared vertex vector of one of `bucket`'s vectors, from the tile's
/// `geometry` when it has one, else a copy.
pub fn bucket_vertices<V: bytemuck::Pod>(
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
pub fn bucket_indexes(
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
pub fn paint_vertices(
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
