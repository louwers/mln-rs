// C++ `Bucket` is an abstract class with data members: the data is
// `BucketBase`, the virtual interface is the `Bucket` trait. Buckets are
// built on the worker thread and then shared with the render thread
// (`std::shared_ptr<Bucket>` in `LayerRenderData`): `BucketPtr = Arc<dyn
// Bucket>`, `Bucket: Send + Sync`. The render thread shares them too: the
// layers of a layout group share their bucket, and a tile's contents, the
// frame's render tiles and symbol placement each hold it. It changes them
// through `&self`, so what it changes is behind locks and atomics: the
// values feature state rewrites (`update`), a symbol bucket's placement
// state, the upload flag, and the geometry it moves out to draw
// (bucket_geometry.rs).

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_style_types::image::ImagePositions;
use mln_tile::TileLayer;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_wire::{Reader, Wire, Writer};

use crate::gfx::context::Context;
use crate::layout::pattern_layout::PatternLayerMap;

pub(crate) type BucketPtr = Arc<dyn Bucket>;

pub(crate) type FeatureStates = mln_util::hash::HashMap<String, mln_style::expression::expression::FeatureState>;
pub type LayerFeatureStates = mln_util::hash::HashMap<String, FeatureStates>;

/// The data members of `Bucket`.
///
/// Port-only: the codec writes what a worker sets (`tile/worker_codec`); the
/// upload flag and the id are the side's that has the bucket.
#[derive(Debug, mln_wire::Wire)]
pub(crate) struct BucketBase {
    #[wire(skip)]
    pub(crate) uploaded: AtomicBool,
    #[wire(skip, default = "SimpleIdentity::new")]
    pub(crate) bucket_id: SimpleIdentity,
}

impl Default for BucketBase {
    fn default() -> Self {
        BucketBase { uploaded: AtomicBool::new(false), bucket_id: SimpleIdentity::new() }
    }
}

/// The error of a polygon with more vertices than a 16-bit index can
/// address. The tile that holds it fails to lay out.
#[must_use]
pub(crate) fn geometry_too_long() -> Error {
    Error::native("a polygon has more vertices than a 16-bit index can address")
}

pub(crate) trait Bucket: Any + Send + Sync {
    fn base(&self) -> &BucketBase;

    /// Feature geometries are also used to populate the feature index.
    /// Obtaining these is a costly operation, so we do it only once, and
    /// pass-by-const-ref the geometries as a second parameter.
    ///
    /// Fails with [`geometry_too_long`] for a polygon that a bucket can't
    /// index; the worker reports it as the tile's error.
    fn add_feature(
        &mut self,
        _feature: &dyn GeometryTileFeature,
        _geometries: &GeometryCollection,
        _image_positions: &ImagePositions,
        _patterns: &PatternLayerMap,
        _index: usize,
        _canonical: &CanonicalTileID,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Re-evaluates the feature-state dependent attribute values of the
    /// layer `layer_id`, and returns whether the bucket has any. Takes
    /// `&self` because the bucket is shared (see the file comment);
    /// implementations lock their binders.
    fn update(
        &self,
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

    /// As long as this bucket has a Prepare render pass, this function is
    /// getting called. Typically, this only happens once when the bucket is
    /// being rendered for the first time.
    fn upload(&self, context: &Context);

    fn has_data(&self) -> bool;

    /// Writes the bucket for its layer type's
    /// [`read_bucket`](crate::layer_type::ErasedLayerType::read_bucket).
    /// A bucket that no layout makes fails `w`.
    fn write_wire(&self, w: &mut Writer);

    fn needs_upload(&self) -> bool {
        self.has_data() && !self.base().uploaded.load(Ordering::SeqCst)
    }

    fn get_id(&self) -> SimpleIdentity {
        self.base().bucket_id
    }
}

/// The bucket of a layer type that doesn't lay out: there is no value.
pub(crate) enum NoBucket {}

impl Bucket for NoBucket {
    fn base(&self) -> &BucketBase {
        match *self {}
    }

    fn upload(&self, _context: &Context) {
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

impl dyn Bucket {
    /// Downcasts to the bucket type `B`; `None` if it is another type.
    pub(crate) fn downcast_ref<B: Bucket>(&self) -> Option<&B> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

/// `bucket` as a `B`, the same allocation, or `None` if it is another type.
pub(crate) fn recover<B: Bucket>(bucket: &BucketPtr) -> Option<Arc<B>> {
    let any: Arc<dyn Any + Send + Sync> = Arc::<dyn Bucket>::clone(bucket);
    any.downcast().ok()
}
