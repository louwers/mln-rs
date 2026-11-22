//! Buckets as the engine holds them: a layer type's [`Bucket`] with what
//! the engine keeps of every bucket ([`Stored`]), as `dyn ErasedBucket`.

use std::any::Any;

use mln_style_types::image::ImagePositions;
use mln_tile::TileLayer;
use mln_util::identity::SimpleIdentity;
use mln_wire::Writer;

use crate::bucket_geometry::BucketGeometry;
use crate::bucket_upload::{Kind, Upload};
use crate::gfx::context::Resources;
use crate::layer::Bucket;

/// The state of each feature of a source layer, by the feature's id.
pub type FeatureStates = mln_util::hash::HashMap<String, mln_style::expression::expr::FeatureState>;
pub type LayerFeatureStates = mln_util::hash::HashMap<String, FeatureStates>;

/// A bucket with what the engine keeps of it: its identity, which tells
/// apart what the tile's layers made of it, and whether its data is in its
/// tile's buffers as it is. A copy keeps the identity: its meshes are the
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

    /// Whether the bucket has data its tile's buffers don't have yet.
    fn needs_upload(&self) -> bool;

    /// Uploads the bucket's data into `geometry`'s buffers in `res`,
    /// sharing those of `copies`.
    fn upload(&mut self, geometry: &mut BucketGeometry, copies: &[&BucketGeometry], res: &mut Resources);

    /// Rewrites the values of the layer `layer_id` that read feature state
    /// for `states`, the states of `layer`'s features, and returns whether
    /// the bucket has any, which then upload again.
    fn update(
        &mut self,
        states: &FeatureStates,
        layer: &TileLayer<'_>,
        layer_id: &str,
        patterns: &ImagePositions,
    ) -> bool;

    /// How far above the ground the bucket's highest content is raised.
    fn max_elevation(&self) -> f32;

    /// A copy, for the tile in another world copy.
    fn clone_bucket(&self) -> Box<dyn ErasedBucket>;

    /// Writes the bucket for its layer type's codec.
    fn write_wire(&self, w: &mut Writer);
}

impl<B: Bucket> ErasedBucket for Stored<B> {
    fn id(&self) -> SimpleIdentity {
        self.id
    }

    fn needs_upload(&self) -> bool {
        !self.uploaded
    }

    fn upload(&mut self, geometry: &mut BucketGeometry, copies: &[&BucketGeometry], res: &mut Resources) {
        self.bucket.upload(&mut Upload { kind: Kind::Geometry { geometry, copies, res } });
        self.uploaded = true;
    }

    fn update(
        &mut self,
        states: &FeatureStates,
        layer: &TileLayer<'_>,
        layer_id: &str,
        _patterns: &ImagePositions,
    ) -> bool {
        let mut upload = Upload { kind: Kind::FeatureState { states, layer, layer_id, rewrote: false } };
        self.bucket.upload(&mut upload);
        let Kind::FeatureState { rewrote, .. } = upload.kind else { unreachable!() };
        if rewrote {
            self.uploaded = false;
        }
        rewrote
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
