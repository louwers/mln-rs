// A `TileLayer` borrows its `TileData`, and the worker keeps layouts with
// pending dependencies across messages, so layouts get a `SourceLayer`: the
// tile data and the layer name, from which they re-borrow the layer (and
// re-get features by index) when they need it.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::expression::UnavailableImages;
use mln_style::tile_id::CanonicalTileID;
use mln_text::glyph::GlyphDependencies;
use mln_vec_map::{VecMap, VecSet};

use crate::image::image_store::ImageDependencies;
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::filter_groups::FilterGroups;
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::tile_layout::Resolved;
use crate::query::feature_index::FeatureIndexBuilder;
use crate::tile::{TileData, TileLayer};

/// The layer a layout reads: its tile data and its name.
#[derive(Clone)]
pub(crate) struct SourceLayer {
    pub(crate) data: TileData,
    pub(crate) name: String,
}

impl SourceLayer {
    pub(crate) fn new(data: TileData, name: impl Into<String>) -> Self {
        SourceLayer { data, name: name.into() }
    }

    /// The layer. The worker only creates layouts for layers the data has,
    /// so neither error is expected.
    pub(crate) fn layer(&self) -> Result<TileLayer<'_>, Error> {
        self.data
            .layer(&self.name)?
            .ok_or_else(|| Error::native(format!("SourceLayer: the tile data has no layer {:?}", self.name)))
    }
}

/// How a layer type's layers become a tile's bucket: made from a layer
/// group and the tile's data, it selects and evaluates the features, and
/// once the glyphs and images it needs are resolved, makes the bucket.
pub(crate) trait Layout<T: LayerType>: Send + 'static {
    /// Whether the bucket needs the glyphs or images the layout recorded.
    fn has_dependencies(&self) -> bool;

    /// Makes the bucket and adds its features to `feature_index`.
    /// `resolved` is what the dependencies resolved to, or `None` for a
    /// layout without dependencies. `None` when the layout has nothing to
    /// draw. A feature that fails to decode fails the bucket; the worker
    /// reports it as the tile's error.
    fn create_bucket(
        &mut self,
        resolved: Option<&Resolved>,
        feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<T::Bucket>, Error>;

    /// The layers the bucket renders for, by id, in group order.
    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<T>>>;
}

/// What making a bucket reads apart from the layout and its dependencies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuildParams {
    /// Whether the tile has had no layout yet.
    pub(crate) first_load: bool,
    pub(crate) canonical: CanonicalTileID,
}

/// The layout of a layer type that doesn't lay out: there is no value.
pub(crate) enum NoLayout {}

impl<T: LayerType> Layout<T> for NoLayout {
    fn has_dependencies(&self) -> bool {
        match *self {}
    }

    fn create_bucket(
        &mut self,
        _resolved: Option<&Resolved>,
        _feature_index: &mut FeatureIndexBuilder,
        _build: &BuildParams,
    ) -> Result<Option<T::Bucket>, Error> {
        match *self {}
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<T>>> {
        match *self {}
    }
}

/// The group's layers by id, the first of each id kept.
pub(crate) fn layers_by_id<T: LayerType>(group: &[Arc<EvaluatedLayer<T>>]) -> VecMap<String, Arc<EvaluatedLayer<T>>> {
    let mut layers = VecMap::new();
    for layer in group {
        layers.entry(layer.style.id.clone()).or_insert_with(|| Arc::clone(layer));
    }
    layers
}

pub(crate) struct LayoutParameters<'a> {
    pub(crate) bucket_parameters: &'a BucketParameters,
    /// The style's font faces, and its fallback faces after them.
    pub(crate) font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    pub(crate) glyph_dependencies: &'a mut GlyphDependencies,
    pub(crate) image_dependencies: &'a mut ImageDependencies,
    pub(crate) available_images: &'a VecSet<String>,
    /// Where `image` expressions record the images not in `available_images`.
    pub(crate) unavailable_images: &'a UnavailableImages,
    /// Port-only: the tile's feature groups, shared by the layouts' filters.
    pub(crate) filter_groups: &'a mut FilterGroups,
}
