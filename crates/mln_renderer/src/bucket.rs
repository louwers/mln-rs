// C++ `Bucket` is an abstract class with data members: the data is
// `BucketBase`, the virtual interface is the `Bucket` trait. Buckets are
// built on the worker thread and then shared with the render thread
// (`std::shared_ptr<Bucket>` in `LayerRenderData`): `BucketPtr = Arc<dyn
// Bucket>`, `Bucket: Send + Sync`. The render thread only reads them, except
// `upload` (an atomic flag, as in C++) and the geometry it moves out to draw
// (bucket_geometry.rs).

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use mln_gfx::UploadPass;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature, GeometryTileLayer};
use mln_util::error::Error;
use mln_util::feature::feature_id_to_string;
use mln_util::identity::SimpleIdentity;
use serde::{Deserialize, Serialize};

use crate::layout::pattern_layout::PatternLayerMap;
use crate::render_layer::RenderLayer;

pub(crate) type BucketPtr = Arc<dyn Bucket>;

pub(crate) type FeatureStates = std::collections::HashMap<String, mln_style::expression::expression::FeatureState>;
pub type LayerFeatureStates = std::collections::HashMap<String, FeatureStates>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RetainedFeature {
    pub(crate) feature_id: String,
    pub(crate) vertex_offset: usize,
    pub(crate) vertex_count: usize,
}

/// The data members of `Bucket`.
///
/// Port-only: serde writes what a worker sets (`tile/worker_codec`); the
/// upload flag, the id and the version are the side's that has the bucket.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BucketBase {
    #[serde(skip)]
    pub(crate) uploaded: AtomicBool,
    #[serde(skip, default = "SimpleIdentity::new")]
    pub(crate) bucket_id: SimpleIdentity,
    pub(crate) retain_features_by_id: bool,
    pub(crate) retained_features: Vec<RetainedFeature>,
    /// Bumped by `update` (feature state); the render layers compare it to
    /// refresh their drawables' copies of the vertex data. C++ updates the
    /// shared vertex vectors in place instead.
    #[serde(skip)]
    pub(crate) version: AtomicU64,
}

impl Default for BucketBase {
    fn default() -> Self {
        BucketBase {
            version: AtomicU64::new(0),
            uploaded: AtomicBool::new(false),
            bucket_id: SimpleIdentity::new(),
            retain_features_by_id: false,
            retained_features: Vec::new(),
        }
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
    fn base_mut(&mut self) -> &mut BucketBase;

    fn reserve_features(&mut self, vertex_count: usize) {
        self.base_mut().retained_features.reserve(vertex_count);
    }

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

    /// Re-evaluates the feature-state dependent attribute values. Takes
    /// `&self` because the bucket is shared (see the file comment);
    /// implementations lock their binders.
    fn update(
        &self,
        _states: &FeatureStates,
        _layer: &dyn GeometryTileLayer,
        _layer_id: &str,
        _image_positions: &ImagePositions,
    ) {
    }

    /// The `version` counter (see `BucketBase::version`).
    fn get_version(&self) -> u64 {
        self.base().version.load(Ordering::SeqCst)
    }

    fn record_sdf_pattern(&mut self, _layer_id: &str, _sdf: bool) {}
    fn is_sdf_pattern(&self, _layer_id: &str) -> bool {
        false
    }

    /// As long as this bucket has a Prepare render pass, this function is
    /// getting called. Typically, this only happens once when the bucket is
    /// being rendered for the first time.
    fn upload(&self, upload_pass: &dyn UploadPass);

    fn has_data(&self) -> bool;

    fn get_query_radius(&self, _layer: &dyn RenderLayer) -> f32 {
        0.0
    }

    fn needs_upload(&self) -> bool {
        self.has_data() && !self.base().uploaded.load(Ordering::SeqCst)
    }

    fn get_id(&self) -> SimpleIdentity {
        self.base().bucket_id
    }

    fn set_retain_features_by_id(&mut self, value: bool) {
        self.base_mut().retain_features_by_id = value;
    }

    fn retain_feature(&mut self, feature: &dyn GeometryTileFeature, vertex_offset: usize, vertex_count: usize) {
        let feature_id = get_retain_feature_id(feature);
        self.base_mut().retained_features.push(RetainedFeature { feature_id, vertex_offset, vertex_count });
    }

    fn get_retained_features(&self) -> &[RetainedFeature] {
        &self.base().retained_features
    }
}

impl dyn Bucket {
    /// Downcasts to the bucket type `B`; `None` if it is another type.
    pub(crate) fn downcast_ref<B: Bucket>(&self) -> Option<&B> {
        (self as &dyn Any).downcast_ref()
    }
}

static UNIDENTIFIED_FEATURE_INDEX: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn get_retain_feature_id(feature: &dyn GeometryTileFeature) -> String {
    if let Some(id_str) = feature_id_to_string(&feature.get_id())
        && !id_str.is_empty()
    {
        return id_str;
    }
    // Assign a unique number to unidentified features
    format!("maplibre:{}", UNIDENTIFIED_FEATURE_INDEX.fetch_add(1, Ordering::SeqCst))
}
