use std::any::Any;

use mln_style_types::image::ImagePositions;
use mln_tile::TileLayer;
use mln_util::identity::SimpleIdentity;
use mln_wire::{Reader, Wire, Writer};

use crate::bucket_geometry::GeometryUpload;

/// The state of each feature of a source layer, by the feature's id.
pub type FeatureStates = mln_util::hash::HashMap<String, mln_style::expression::expression::FeatureState>;
pub type LayerFeatureStates = mln_util::hash::HashMap<String, FeatureStates>;

/// What every bucket keeps for the engine: its identity, and whether its
/// geometry is in its tile's draws as it is.
///
/// The codec writes neither: a bucket read from a worker is new to the side
/// that reads it. A copy keeps the identity: its draws are the same.
#[derive(Clone, Debug, mln_wire::Wire)]
pub struct BucketBase {
    #[wire(skip)]
    pub(crate) uploaded: bool,
    #[wire(skip, default = "SimpleIdentity::new")]
    pub(crate) bucket_id: SimpleIdentity,
}

impl Default for BucketBase {
    fn default() -> Self {
        BucketBase { uploaded: false, bucket_id: SimpleIdentity::new() }
    }
}

impl BucketBase {
    /// The bucket's identity, which tells its draws' GPU objects apart.
    #[must_use]
    pub fn id(&self) -> SimpleIdentity {
        self.bucket_id
    }

    /// Records whether the bucket's geometry is in its tile's draws as it
    /// is: true once [`Bucket::upload`] moved it there, false once the bucket
    /// changed what its draws read, for the engine to upload it again.
    pub fn set_uploaded(&mut self, uploaded: bool) {
        self.uploaded = uploaded;
    }
}

/// What the engine does with a bucket, whatever its type: the vertices,
/// indices and paint values that a tile's layout made of a layer's features
/// for the layer's draws.
///
/// Workers build buckets and send them, through the codec, to the render
/// thread, which owns each one in its tile's contents: it rewrites a
/// bucket's feature-state values ([`update`](Self::update)), moves its
/// geometry into the stores that the tile's draws share
/// ([`upload`](Self::upload)), and copies it for the tile's other world
/// copies. What a type does besides is its own.
pub trait Bucket: Any + Send + BucketValue {
    /// What the engine keeps of every bucket: its identity and whether it
    /// uploaded.
    fn base(&self) -> &BucketBase;

    /// Re-evaluates the feature-state dependent paint values of the layer
    /// `layer_id` for `states`, and returns whether the bucket has any. A
    /// bucket that rewrote some records that it needs an upload
    /// ([`BucketBase::set_uploaded`]).
    fn update(
        &mut self,
        _states: &FeatureStates,
        _layer: &TileLayer<'_>,
        _layer_id: &str,
        _image_positions: &ImagePositions,
    ) -> bool {
        false
    }

    /// Readies the bucket for its tile's draws, once it has data and until
    /// it changes again: moves its vertices and indices into the context's
    /// stores through `geometry`, for the draws to share, all but the values
    /// that feature state rewrites, which stay with the bucket, and records
    /// that it did ([`BucketBase::set_uploaded`]).
    fn upload(&mut self, geometry: &mut GeometryUpload<'_>);

    /// Whether the bucket has anything to draw. A layout keeps no bucket
    /// without.
    fn has_data(&self) -> bool;
}

impl dyn Bucket {
    /// Whether the bucket has data its tile's draws don't have yet.
    pub fn needs_upload(&self) -> bool {
        !self.base().uploaded && self.has_data()
    }

    pub(crate) fn id(&self) -> SimpleIdentity {
        self.base().bucket_id
    }

    /// Downcasts to the bucket type `B`; `None` if it is another type.
    #[cfg(test)]
    pub(crate) fn downcast_ref<B: Bucket>(&self) -> Option<&B> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

/// What the engine does with a bucket as a value: copies it, for the tile
/// in another world copy, which draws the same geometry, and writes it for
/// its layer type's codec. Every bucket that is `Clone` and [`Wire`] has it.
pub trait BucketValue {
    /// The bucket's copy: what the bucket moved to its tile's geometry, the
    /// copy finds in a copy of that.
    fn clone_bucket(&self) -> Box<dyn Bucket>;

    /// Writes the bucket, which the layer type's codec reads as its
    /// [`LayerType::Bucket`](crate::layer::LayerType::Bucket).
    fn write_wire(&self, w: &mut Writer);
}

impl<B: Bucket + Clone + Wire> BucketValue for B {
    fn clone_bucket(&self) -> Box<dyn Bucket> {
        Box::new(self.clone())
    }

    fn write_wire(&self, w: &mut Writer) {
        self.write(w);
    }
}

/// The bucket of a layer type that doesn't lay out: there is no value.
#[derive(Clone)]
pub enum NoBucket {}

impl Bucket for NoBucket {
    fn base(&self) -> &BucketBase {
        match *self {}
    }

    fn upload(&mut self, _geometry: &mut GeometryUpload<'_>) {
        match *self {}
    }

    fn has_data(&self) -> bool {
        match *self {}
    }
}

impl Wire for NoBucket {
    fn write(&self, _w: &mut Writer) {
        match *self {}
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        r.fail("a bucket of a layer type that doesn't lay out")
    }
}

// The benchmarks (`bench.rs`) hold the bucket a benchmark's layer places
// through this path, which they take in every commit they compare.
#[cfg(feature = "bench")]
pub(crate) use crate::render_layer::hold_bucket as recover;
