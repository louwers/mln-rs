// A `TileLayer` borrows its `TileData`, and the worker keeps layouts with
// pending dependencies across messages, so layouts get a `SourceLayer`: the
// tile data and the layer name, from which they re-borrow the layer (and
// re-get features by index) when they need it.

use std::sync::Arc;

use mln_style::expression::image_expression::UnavailableImages;
use mln_style_types::image::ImageDependencies;
use mln_text::glyph::GlyphDependencies;
use mln_tile::{TileData, TileLayer};
use mln_tile_id::CanonicalTileID;
use mln_util::error::Error;
use mln_vec_map::{VecMap, VecSet};

use crate::bucket_parameters::BucketParameters;
use crate::geometry::feature_index::FeatureIndexBuilder;
use crate::layer::{EvaluatedLayer, LayerType};
use crate::layout::filter_groups::FilterGroups;
use crate::layout::tile_layout::Resolved;

/// The layer a layout reads: its tile data and its name.
#[derive(Clone)]
pub struct SourceLayer {
    pub(crate) data: TileData,
    pub(crate) name: String,
}

impl SourceLayer {
    /// The layer `name` of `data`.
    pub fn new(data: TileData, name: impl Into<String>) -> Self {
        SourceLayer { data, name: name.into() }
    }

    /// The layer's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The layer. The worker only creates layouts for layers the data has,
    /// so neither error is expected.
    pub fn layer(&self) -> Result<TileLayer<'_>, Error> {
        self.data
            .layer(&self.name)?
            .ok_or_else(|| Error::native(format!("SourceLayer: the tile data has no layer {:?}", self.name)))
    }
}

/// How a layer type's layers become a tile's bucket: made from a layer
/// group and the tile's data, it selects and evaluates the features, and
/// once the glyphs and images it needs are resolved, makes the bucket.
pub trait Layout<T: LayerType>: Send + 'static {
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
pub struct BuildParams {
    /// Whether the tile has had no layout yet.
    pub first_load: bool,
    /// Whether the map shows collision boxes, which only the types that
    /// place read.
    pub show_collision_boxes: bool,
    /// The tile the bucket is of.
    pub canonical: CanonicalTileID,
}

/// The layout of a layer type that doesn't lay out: there is no value.
pub enum NoLayout {}

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
pub fn layers_by_id<T: LayerType>(group: &[Arc<EvaluatedLayer<T>>]) -> VecMap<String, Arc<EvaluatedLayer<T>>> {
    let mut layers = VecMap::new();
    for layer in group {
        layers.entry(layer.style.id.clone()).or_insert_with(|| Arc::clone(layer));
    }
    layers
}

/// What a tile's layout reads, and where it records the glyphs and images
/// its layers need.
pub struct LayoutParameters<'a> {
    /// The tile the layout lays out, and how.
    pub bucket_parameters: &'a BucketParameters,
    /// The style's font faces, and its fallback faces after them.
    pub font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub glyph_pbfs: bool,
    /// Where a layout records the glyphs it needs.
    pub glyph_dependencies: &'a mut GlyphDependencies,
    /// Where a layout records the images it needs.
    pub image_dependencies: &'a mut ImageDependencies,
    /// The images of the style.
    pub available_images: &'a VecSet<String>,
    /// Where `image` expressions record the images not in `available_images`.
    pub unavailable_images: &'a UnavailableImages,
    /// The tile's feature groups, shared by the layouts' filters.
    pub filter_groups: &'a mut FilterGroups,
}
