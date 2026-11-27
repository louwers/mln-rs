//! Buckets: what a tile's layout makes of a layer group's features for the
//! layers' draws, their vertices, indices and paint values. Workers build
//! buckets and send them to the render thread, which owns each one once, in
//! its tile's contents, as a `Stored` bucket behind `dyn ErasedBucket`.
//! A layer type's `Bucket` names its data to an `Upload`; the engine
//! keeps the rest: the bucket's identity, whether it uploaded, and the
//! feature-state rewrites of its paint values.

use std::any::Any;

use mln_gfx::gpu::Caps;
use mln_wire::{Wire, Writer};

use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::layer::bucket_geometry::{BucketGeometry, GeometryUpload, GeometryVec};
use crate::layer::paint_property_binder::{LayerBinders, PaintBinders};
use crate::tile::TileLayer;

/// The state of each feature of a source layer, by the feature's id.
pub(crate) type FeatureStates = mln_style::hash::HashMap<String, mln_style::feature::FeatureState>;
pub type LayerFeatureStates = mln_style::hash::HashMap<String, FeatureStates>;

/// What a layout makes of a tile for a group of layers: the vertices,
/// indices and paint values that their draws read.
///
/// The engine moves a bucket from the worker to the render thread, copies
/// it for the tile's other world copies, and uploads it.
pub(crate) trait Bucket: Clone + Send + Wire + 'static {
    /// Names the bucket's [`GeometryVec`]s and [`LayerBinders`] to `up`: the
    /// engine moves them into the context's stores once the bucket reaches
    /// the render thread, and rewrites the binders' values where feature
    /// state changes.
    fn upload(&mut self, up: &mut Upload<'_>);

    /// How far above the ground, in meters, the bucket's highest content
    /// is raised, for a layer whose content's elevation is an expression.
    fn max_elevation(&self) -> f32 {
        0.0
    }
}

/// What a bucket names its data to, in its [`upload`](Bucket::upload).
pub(crate) struct Upload<'a> {
    kind: Kind<'a>,
}

enum Kind<'a> {
    /// Moves the data into the context's stores.
    Geometry(GeometryUpload<'a>),
    /// Rewrites the binders of the layer `layer_id` for `states`, the
    /// states of `layer`'s features, and records whether any read them.
    FeatureState { states: &'a FeatureStates, layer: &'a TileLayer<'a>, layer_id: &'a str, rewrote: bool },
}

#[cfg(test)]
impl<'a> Upload<'a> {
    /// The upload of a bucket whose geometry `geometry` keeps, into
    /// `context`'s stores, sharing what `copies` keep.
    pub(crate) fn geometry(
        geometry: &'a mut BucketGeometry,
        copies: &'a [&'a BucketGeometry],
        context: &'a mut Context,
    ) -> Self {
        Upload { kind: Kind::Geometry(GeometryUpload::new(geometry, copies, context)) }
    }
}

impl Upload<'_> {
    /// What the context's device allows, where the upload moves data: a
    /// bucket names only the vectors its draws read there. `None` where the
    /// upload rewrites the binders' values that read feature state, and
    /// moves nothing.
    #[must_use]
    pub(crate) fn caps(&self) -> Option<&Caps> {
        match &self.kind {
            Kind::Geometry(geometry) => Some(geometry.caps()),
            Kind::FeatureState { .. } => None,
        }
    }

    /// Names `vec`, vertices that the bucket's draws read
    /// ([`bucket_vertices`](crate::layer::bucket_geometry::bucket_vertices)).
    pub(crate) fn vec<V: bytemuck::Pod>(&mut self, vec: &mut GeometryVec<V>) {
        if let Kind::Geometry(geometry) = &mut self.kind {
            geometry.move_vertices(vec);
        }
    }

    /// Names `indexes`, which the bucket's draws draw
    /// ([`bucket_indexes`](crate::layer::bucket_geometry::bucket_indexes)).
    pub(crate) fn indexes(&mut self, indexes: &mut GeometryVec<u16>) {
        if let Kind::Geometry(geometry) = &mut self.kind {
            geometry.move_indexes(indexes);
        }
    }

    /// Keeps the vertices that `make` makes of the data of `vec`, for
    /// [`derived_vertices`](crate::layer::bucket_geometry::derived_vertices).
    /// Before `vec` is named.
    pub(crate) fn derive<V: bytemuck::Pod, D: bytemuck::Pod>(
        &mut self,
        vec: &GeometryVec<V>,
        make: impl FnOnce(&[V]) -> Vec<D>,
    ) {
        if let Kind::Geometry(geometry) = &mut self.kind {
            geometry.derive_vertices(vec, make);
        }
    }

    /// Names `binders`, whose values the bucket's draws read
    /// ([`paint_vertices`](crate::layer::bucket_geometry::paint_vertices)).
    /// Values that feature state rewrites stay with the bucket.
    pub(crate) fn binders<P: PaintBinders>(&mut self, binders: &mut LayerBinders<P>) {
        for (layer_id, binders) in binders.iter_mut() {
            self.paint(layer_id, binders);
        }
    }

    /// Names `binders`, the layer `layer_id`'s, as [`binders`](Self::binders)
    /// names each layer's, for a bucket that keeps a layer's binders its own
    /// way.
    pub(crate) fn paint<P: PaintBinders>(&mut self, layer_id: &str, binders: &mut P) {
        let depends_on_feature_state = binders.depends_on_feature_state();
        match &mut self.kind {
            Kind::Geometry(geometry) => geometry.move_paint(binders.vertices_mut(), depends_on_feature_state),
            Kind::FeatureState { states, layer, layer_id: changed, rewrote } => {
                if layer_id == *changed && depends_on_feature_state {
                    binders.feature_state(states, layer);
                    *rewrote = true;
                }
            }
        }
    }
}

/// A bucket with what the engine keeps of it: its identity, which tells
/// apart what the tile's layers made of it, and whether its data is in its
/// tile's stores as it is. A copy keeps the identity: its draws are the
/// same. The worker codec writes the bucket alone, so a bucket read from a
/// worker is new to the side that reads it.
#[derive(Clone)]
pub(crate) struct Stored<B> {
    pub(crate) bucket: B,
    id: SimpleIdentity,
    uploaded: bool,
}

impl<B> Stored<B> {
    pub(crate) fn new(bucket: B) -> Self {
        Stored { bucket, id: SimpleIdentity::new(), uploaded: false }
    }
}

/// What the engine does with a bucket, whatever its type.
pub(crate) trait ErasedBucket: Any + Send {
    /// The bucket's identity.
    fn id(&self) -> SimpleIdentity;

    /// Whether the bucket has data its tile's stores don't have yet.
    fn needs_upload(&self) -> bool;

    /// Moves the bucket's data into `geometry`'s stores in `context`,
    /// sharing what `copies`, the geometry of the tile's other world
    /// copies, keep of it.
    fn upload(&mut self, geometry: &mut BucketGeometry, copies: &[&BucketGeometry], context: &mut Context);

    /// Rewrites the values of the layer `layer_id` that read feature state
    /// for `states`, the states of `layer`'s features. Returns whether the
    /// bucket has any.
    fn set_feature_state(&mut self, states: &FeatureStates, layer: &TileLayer<'_>, layer_id: &str) -> bool;

    /// [`Bucket::max_elevation`].
    fn max_elevation(&self) -> f32;

    /// A copy, for the tile in another world copy.
    fn clone_bucket(&self) -> Box<dyn ErasedBucket>;

    /// Writes the bucket for its layer type's
    /// [`read_bucket`](crate::layer::layer_type::ErasedLayerType::read_bucket).
    fn write_wire(&self, w: &mut Writer);
}

impl<B: Bucket> ErasedBucket for Stored<B> {
    fn id(&self) -> SimpleIdentity {
        self.id
    }

    fn needs_upload(&self) -> bool {
        !self.uploaded
    }

    fn upload(&mut self, geometry: &mut BucketGeometry, copies: &[&BucketGeometry], context: &mut Context) {
        let geometry = GeometryUpload::new(geometry, copies, context);
        self.bucket.upload(&mut Upload { kind: Kind::Geometry(geometry) });
        self.uploaded = true;
    }

    fn set_feature_state(&mut self, states: &FeatureStates, layer: &TileLayer<'_>, layer_id: &str) -> bool {
        let mut upload = Upload { kind: Kind::FeatureState { states, layer, layer_id, rewrote: false } };
        self.bucket.upload(&mut upload);
        matches!(upload.kind, Kind::FeatureState { rewrote: true, .. })
    }

    fn max_elevation(&self) -> f32 {
        self.bucket.max_elevation()
    }

    fn clone_bucket(&self) -> Box<dyn ErasedBucket> {
        Box::new(self.clone())
    }

    fn write_wire(&self, w: &mut Writer) {
        self.bucket.write(w);
    }
}

impl dyn ErasedBucket {
    /// The bucket as a `B`; `None` for one of another type.
    pub(crate) fn downcast_ref<B: Bucket>(&self) -> Option<&B> {
        let any: &dyn Any = self;
        Some(&any.downcast_ref::<Stored<B>>()?.bucket)
    }

    /// [`downcast_ref`](Self::downcast_ref), to change.
    pub(crate) fn downcast_mut<B: Bucket>(&mut self) -> Option<&mut B> {
        let any: &mut dyn Any = self;
        Some(&mut any.downcast_mut::<Stored<B>>()?.bucket)
    }
}
