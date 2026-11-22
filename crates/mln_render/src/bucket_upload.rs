//! What a bucket names its GPU data to ([`Upload`]).

use mln_tile::TileLayer;

use crate::bucket::FeatureStates;
use crate::bucket_geometry::{BucketGeometry, GpuVec};
use crate::gfx::context::Resources;
use crate::gfx::mesh::Indexes;
use crate::paint_property_binder::{LayerBinders, PaintBinders};

/// What a bucket names its [`GpuVec`]s and [`LayerBinders`] to, in its
/// [`upload`](crate::layer::Bucket::upload): the engine uploads them into
/// buffers when the bucket reaches the renderer's thread, and rewrites the
/// binders' values that read feature state when it changes.
pub struct Upload<'a> {
    pub(crate) kind: Kind<'a>,
}

pub(crate) enum Kind<'a> {
    /// Uploads into `geometry`'s buffers, sharing those of `copies`, the
    /// geometry of the tile's other world copies.
    Geometry { geometry: &'a mut BucketGeometry, copies: &'a [&'a BucketGeometry], res: &'a mut Resources },
    /// Rewrites the binders of the layer `layer_id` for `states`, the
    /// states of `layer`'s features, and records whether any read them.
    FeatureState { states: &'a FeatureStates, layer: &'a TileLayer<'a>, layer_id: &'a str, rewrote: bool },
}

impl Upload<'_> {
    /// Names `vec`, which the bucket's meshes read through its buffer
    /// ([`LayerBucket::buffer`](crate::layer::LayerBucket::buffer)).
    pub fn vec<V: bytemuck::Pod>(&mut self, vec: &mut GpuVec<V>) {
        if let Kind::Geometry { geometry, copies, res } = &mut self.kind {
            geometry.upload(vec, false, copies, res);
        }
    }

    /// Names `indexes`, which the bucket's meshes draw through their buffer
    /// ([`LayerBucket::indexes`](crate::layer::LayerBucket::indexes)).
    pub fn indexes(&mut self, indexes: &mut Indexes) {
        if let Kind::Geometry { geometry, copies, res } = &mut self.kind {
            geometry.upload_indexes(indexes, copies, res);
        }
    }

    /// Names `binders`, whose values the bucket's meshes read
    /// ([`LayerBucket::binders`](crate::layer::LayerBucket::binders)).
    pub fn binders<P: PaintBinders>(&mut self, binders: &mut LayerBinders<P>) {
        match &mut self.kind {
            Kind::Geometry { geometry, copies, res } => {
                for (_, binders) in binders.iter_mut() {
                    // Values that feature state rewrites stay, and upload
                    // again into the same buffer.
                    let keep = binders.depends_on_feature_state();
                    geometry.upload(binders.vertices_mut().vertices_mut(), keep, copies, res);
                }
            }
            Kind::FeatureState { states, layer, layer_id, rewrote } => {
                for (id, binders) in binders.iter_mut() {
                    if id == *layer_id && binders.depends_on_feature_state() {
                        binders.feature_state(states, layer);
                        *rewrote = true;
                    }
                }
            }
        }
    }
}
