// A bucket is what a tile's layout makes of a layer's features for the
// layer's draws: their vertices, indices and paint values. Workers build
// buckets and send them to the render thread, which owns each one once, in
// its tile's contents, and changes it only where it has the contents
// mutably: feature state and symbol placement in a frame's update, the
// upload that moves its geometry to the draws in the frame's encode. What
// every bucket has is `BucketBase`; what each type does, the `Bucket` trait.

use std::any::Any;

use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::tile_id::CanonicalTileID;
use mln_text::atlas::ImagePositions;
use mln_tile::TileLayer;
use mln_util::identity::SimpleIdentity;
use mln_wire::{Reader, Wire, Writer};

use crate::layer::bucket_geometry::GeometryUpload;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::util::tessellator::Tessellator;

pub(crate) type FeatureStates = mln_style::hash::HashMap<String, mln_style::feature::FeatureState>;
pub type LayerFeatureStates = mln_style::hash::HashMap<String, FeatureStates>;

/// What every bucket has, whatever its type.
///
/// The codec writes what a worker sets (`tile/worker_codec`); the upload flag
/// and the id are the side's that has the bucket. A copy keeps the id: its
/// draws are the same.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct BucketBase {
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

/// What the frame's draws read of the buckets apart from their own
/// geometry, which depends on the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UploadNeeds {
    /// Whether fill outlines draw as instanced quads, which read the fill
    /// buckets' outline ends.
    pub(crate) fill_outline_ends: bool,
    /// Whether fill-extrusion walls draw as quads of their own, which their
    /// buckets make of their vertices, rather than as instances of a static
    /// quad.
    pub(crate) wall_quads: bool,
}

pub(crate) trait Bucket: Any + Send + CloneBucket {
    fn base(&self) -> &BucketBase;

    /// Feature geometries are also used to populate the feature index.
    /// Obtaining these is a costly operation, so we do it only once, and
    /// pass-by-const-ref the geometries as a second parameter.
    ///
    /// A bucket that meshes lines or polygons keeps its scratch in
    /// `tessellator`.
    #[expect(clippy::too_many_arguments, reason = "the feature and what the layout lays it out with")]
    fn add_feature(
        &mut self,
        _feature: &dyn GeometryTileFeature,
        _geometries: &GeometryCollection,
        _image_positions: &ImagePositions,
        _patterns: &PatternLayerMap,
        _index: usize,
        _canonical: &CanonicalTileID,
        _tessellator: &mut Tessellator,
    ) {
    }

    /// Re-evaluates the feature-state dependent attribute values of the
    /// layer `layer_id`, and returns whether the bucket has any.
    fn update(
        &mut self,
        _states: &FeatureStates,
        _layer: &TileLayer<'_>,
        _layer_id: &str,
        _image_positions: &ImagePositions,
    ) -> bool {
        false
    }

    /// How far above the ground the bucket's highest content is raised,
    /// for a layer whose content elevation is an expression.
    fn max_content_elevation(&self) -> f32 {
        0.0
    }

    fn record_sdf_pattern(&mut self, _layer_id: &str, _sdf: bool) {}
    fn is_sdf_pattern(&self, _layer_id: &str) -> bool {
        false
    }

    /// Readies the bucket for its tile's draws, once it has data and until
    /// it changes again: moves its vertices and indices into the context's
    /// stores through `geometry`, for the draws to share, all but the values
    /// that feature state rewrites, which stay with the bucket. What else
    /// the draws read, `needs` says.
    fn upload(&mut self, geometry: &mut GeometryUpload<'_>, needs: UploadNeeds);

    fn has_data(&self) -> bool;

    /// Writes the bucket for its layer type's
    /// [`read_bucket`](crate::layer::layer_type::ErasedLayerType::read_bucket).
    /// A bucket that no layout makes fails `w`.
    fn write_wire(&self, w: &mut Writer);

    fn needs_upload(&self) -> bool {
        !self.base().uploaded && self.has_data()
    }

    fn id(&self) -> SimpleIdentity {
        self.base().bucket_id
    }
}

/// A bucket's copy, for the tile in another world copy, which draws the
/// same geometry: what the bucket moved to its tile's geometry, the copy
/// finds in a copy of that.
pub(crate) trait CloneBucket {
    fn clone_bucket(&self) -> Box<dyn Bucket>;
}

impl<B: Bucket + Clone> CloneBucket for B {
    fn clone_bucket(&self) -> Box<dyn Bucket> {
        Box::new(self.clone())
    }
}

/// The bucket of a layer type that doesn't lay out: there is no value.
#[derive(Clone)]
pub(crate) enum NoBucket {}

impl Bucket for NoBucket {
    fn base(&self) -> &BucketBase {
        match *self {}
    }

    fn upload(&mut self, _geometry: &mut GeometryUpload<'_>, _needs: UploadNeeds) {
        match *self {}
    }

    fn has_data(&self) -> bool {
        match *self {}
    }

    fn write_wire(&self, _w: &mut Writer) {
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

#[cfg(test)]
impl dyn Bucket {
    /// Downcasts to the bucket type `B`; `None` if it is another type.
    pub(crate) fn downcast_ref<B: Bucket>(&self) -> Option<&B> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

// The benchmarks (`bench.rs`) hold the bucket a benchmark's layer places
// through this path, which they take in every commit they compare.
#[cfg(feature = "bench")]
pub(crate) use crate::layer::render_layer::hold_symbol_bucket as recover;
