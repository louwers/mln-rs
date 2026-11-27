//! How a layer type lays out a tile's features on a worker: its [`Layout`],
//! which selects the features with a [`LayoutCx`] and builds the bucket
//! with a [`BuildCx`].
//!
//! A `TileLayer` borrows its `TileData`, and the worker keeps layouts with
//! pending dependencies across messages, so layouts keep the indexes of the
//! features they select, and the contexts borrow the layer again from the
//! engine's [`SourceLayer`] when they run.

use std::sync::Arc;

use mln_style::PropertyEvaluationParameters;
use mln_style::collections::{VecMap, VecSet};
use mln_style::error::Error;
use mln_style::expression::{EvaluationContext, GlobalStateMap, UnavailableImages, Value};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::filter::Filter;
use mln_style::layer::{PaintSet, StyleLayerType};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;

use crate::image::image_store::{ImageDependencies, ImageType};
use crate::layer::bucket::Bucket;
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::filter_groups::FilterGroups;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::{LayerBinders, PaintBinders, PatternLayerMap};
use crate::layer::tile_layout::Resolved;
use crate::map::MapMode;
use crate::query::feature_index::FeatureIndexBuilder;
use crate::text::atlas::ImagePositions;
use crate::text::glyph::{FontFaces, GlyphDependencies};
use crate::tile::{OverscaledTileID, TileData, TileLayer};

/// How a group of layers of the type `T` that share their layout becomes a
/// tile's bucket, on a tile worker: [`new`](Self::new) selects the tile's
/// features, and once the glyphs and images it asked for are resolved,
/// [`build`](Self::build) makes the bucket.
pub trait Layout<T: StyleLayerType>: Send + Sized + 'static {
    /// The bucket the layout makes.
    type Bucket: Bucket;

    /// Whether the layout of `group` may ask for glyphs or images. The
    /// engine makes such a layout before it resolves them, and builds it
    /// once they are, after every other.
    fn needs_resources(_group: &[Arc<EvaluatedLayer<T>>]) -> bool {
        false
    }

    /// Selects the features of the tile that the bucket draws.
    fn new(cx: &mut LayoutCx<'_, T>) -> Result<Self, Error>;

    /// Makes the bucket of the selected features; `None` for one with
    /// nothing to draw. A feature that fails to decode fails the bucket,
    /// which the engine reports as the tile's error.
    fn build(self, cx: &mut BuildCx<'_, T>) -> Result<Option<Self::Bucket>, Error>;
}

/// The layer a layout reads: its tile data and its name.
#[derive(Clone)]
pub struct SourceLayer {
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

/// What a tile's layouts read apart from their group, and where they record
/// the glyphs and images they need.
pub struct LayoutParameters<'a> {
    pub(crate) bucket_parameters: &'a BucketParameters,
    /// The style's font faces, and its fallback faces after them.
    pub(crate) font_faces: Option<&'a Arc<FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    pub(crate) glyph_dependencies: &'a mut GlyphDependencies,
    pub(crate) image_dependencies: &'a mut ImageDependencies,
    pub(crate) available_images: &'a VecSet<String>,
    /// Where `image` expressions record the images not in `available_images`.
    pub(crate) unavailable_images: &'a UnavailableImages,
    /// The tile's feature groups, shared by the layouts' filters.
    pub(crate) filter_groups: &'a mut FilterGroups,
}

/// What a layout reads of its tile while it selects features.
pub struct LayoutCx<'a, T: StyleLayerType> {
    pub(crate) layers: &'a [Arc<EvaluatedLayer<T>>],
    pub(crate) source_layer: &'a SourceLayer,
    pub(crate) parameters: LayoutParameters<'a>,
}

impl<'a, T: StyleLayerType> LayoutCx<'a, T> {
    /// The layers of the group, its leader first, whose style the layout's
    /// layout properties and filter are.
    #[must_use]
    pub fn layers(&self) -> &'a [Arc<EvaluatedLayer<T>>] {
        self.layers
    }

    /// The tile.
    #[must_use]
    pub fn tile(&self) -> OverscaledTileID {
        self.parameters.bucket_parameters.tile_id
    }

    /// The tile's zoom.
    #[must_use]
    pub fn zoom(&self) -> f32 {
        f32::from(self.tile().overscaled_z)
    }

    /// How the map renders.
    #[must_use]
    pub(crate) fn mode(&self) -> MapMode {
        self.parameters.bucket_parameters.mode
    }

    /// The device pixels per logical pixel that the map draws at.
    #[must_use]
    pub(crate) fn pixel_ratio(&self) -> f32 {
        self.parameters.bucket_parameters.pixel_ratio
    }

    /// The style's global state, which `global-state` expressions read.
    #[must_use]
    pub(crate) fn global_state(&self) -> Option<&'a Arc<GlobalStateMap>> {
        self.parameters.bucket_parameters.global_state.as_ref()
    }

    /// What the layout properties evaluate with: the tile's zoom and the
    /// style's global state.
    #[must_use]
    pub fn parameters(&self) -> PropertyEvaluationParameters {
        let mut parameters = PropertyEvaluationParameters::new(self.zoom());
        parameters.global_state = self.global_state().cloned();
        parameters
    }

    /// The style's font faces, which draw text in the scripts of their
    /// ranges, and its fallback faces after them.
    #[must_use]
    pub(crate) fn font_faces(&self) -> Option<&'a Arc<FontFaces>> {
        self.parameters.font_faces
    }

    /// Whether the style has glyphs of its own, a `glyphs` URL.
    #[must_use]
    pub(crate) fn has_glyphs(&self) -> bool {
        self.parameters.glyph_pbfs
    }

    /// The images that exist, which `image` expressions branch on.
    #[must_use]
    pub fn available_images(&self) -> &'a VecSet<String> {
        self.parameters.available_images
    }

    /// Where `image` expressions record the images they find unavailable.
    #[must_use]
    pub fn unavailable_images(&self) -> &'a UnavailableImages {
        self.parameters.unavailable_images
    }

    /// The tile's source layer.
    pub fn source_layer(&self) -> Result<TileLayer<'a>, Error> {
        self.source_layer.layer()
    }

    /// What the leader's filter evaluates with apart from the feature: the
    /// tile's zoom and id and the style's global state.
    #[must_use]
    pub fn filter_context(&self) -> EvaluationContext<'a> {
        let parameters = self.parameters.bucket_parameters;
        EvaluationContext::new(self.zoom())
            .with_canonical_tile_id(Some(&parameters.tile_id.canonical))
            .with_global_state(parameters.global_state.as_deref())
    }

    /// The features of `layer`, the tile's source layer, that the leader's
    /// filter selects, evaluated with `context` ([`filter_context`](Self::filter_context)).
    pub fn select(&mut self, layer: &TileLayer<'_>, context: EvaluationContext<'a>) -> Selection<'a> {
        let leader = &self.layers[0].style;
        let filter = &leader.filter;
        let passes = |feature: &dyn GeometryTileFeature| {
            filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
        };
        let selected = self.parameters.filter_groups.select(&leader.source_layer, layer, filter, &context, passes);
        Selection { selected, filter, context }
    }

    /// Asks for the style's image `id`, as `kind`, unless it is asked for.
    pub fn need_image(&mut self, id: &str, kind: ImageType) {
        if !self.parameters.image_dependencies.contains_key(id) {
            self.parameters.image_dependencies.insert(id.to_owned(), kind);
        }
    }

    /// The glyphs and shapings the layout asks for.
    pub(crate) fn glyphs(&mut self) -> &mut GlyphDependencies {
        self.parameters.glyph_dependencies
    }
}

/// The features of a tile's source layer that a filter selects
/// ([`LayoutCx::select`]).
pub struct Selection<'a> {
    /// Whether the filter selects each feature, where feature groups
    /// answered it; `None` where it evaluates each feature.
    selected: Option<Vec<bool>>,
    filter: &'a Filter,
    context: EvaluationContext<'a>,
}

impl Selection<'_> {
    /// The indexes of the selected features of `layer`, in feature order,
    /// decoding only those that the filter evaluates.
    pub fn indexes(&self, layer: &TileLayer<'_>) -> Result<Vec<usize>, Error> {
        let mut indexes = Vec::new();
        for i in 0..layer.len() {
            let selects = match &self.selected {
                Some(selected) => selected[i],
                None => self.filter.evaluate(&EvaluationContext { feature: Some(&layer.feature(i)?), ..self.context }),
            };
            if selects {
                indexes.push(i);
            }
        }
        Ok(indexes)
    }

    /// Whether the filter may select the feature at `index`: false for one
    /// that its feature group's evaluation rejected, which needn't decode.
    #[inline]
    #[must_use]
    pub fn may_select(&self, index: usize) -> bool {
        self.selected.as_ref().is_none_or(|selected| selected[index])
    }

    /// Whether the filter selects `feature`, a feature that it
    /// [`may_select`](Self::may_select).
    #[inline]
    #[must_use]
    pub fn selects(&self, feature: &dyn GeometryTileFeature) -> bool {
        self.selected.is_some() || self.filter.evaluate(&EvaluationContext { feature: Some(feature), ..self.context })
    }
}

/// What a layout reads and writes while it builds its bucket.
pub struct BuildCx<'a, T: StyleLayerType> {
    pub(crate) layers: &'a [Arc<EvaluatedLayer<T>>],
    pub(crate) parameters: &'a BucketParameters,
    pub(crate) first_load: bool,
    pub(crate) source_layer: &'a SourceLayer,
    pub(crate) resolved: Option<&'a Resolved>,
    pub(crate) feature_index: &'a mut FeatureIndexBuilder,
}

impl<'a, T: StyleLayerType> BuildCx<'a, T> {
    /// The group's layers by id, the first of each id kept.
    #[must_use]
    pub fn layers_by_id(&self) -> VecMap<String, Arc<EvaluatedLayer<T>>> {
        layers_by_id(self.layers)
    }

    /// The tile.
    #[must_use]
    pub fn tile(&self) -> OverscaledTileID {
        self.parameters.tile_id
    }

    /// The tile's id, without its overscaling.
    #[must_use]
    pub fn canonical(&self) -> &'a CanonicalTileID {
        &self.parameters.tile_id.canonical
    }

    /// The tile's zoom.
    #[must_use]
    pub(crate) fn zoom(&self) -> f32 {
        f32::from(self.parameters.tile_id.overscaled_z)
    }

    /// How the map renders.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.parameters.mode
    }

    /// Whether the tile had no layout before this one.
    #[must_use]
    pub(crate) fn first_load(&self) -> bool {
        self.first_load
    }

    /// How finely geometry splits to follow the globe's curve.
    #[must_use]
    pub fn subdivision_granularity(&self) -> SubdivisionGranularitySetting {
        self.parameters.subdivision_granularity
    }

    /// What the glyphs and images the layout asked for resolved to; `None`
    /// for a layout that asked for none.
    #[must_use]
    pub(crate) fn resolved(&self) -> Option<&'a Resolved> {
        self.resolved
    }

    /// Where the resolved patterns are, or none.
    #[must_use]
    pub fn pattern_positions(&self) -> &'a ImagePositions {
        static NONE: std::sync::LazyLock<ImagePositions> = std::sync::LazyLock::new(ImagePositions::default);
        self.resolved.map_or(&NONE, |resolved| &resolved.patterns)
    }

    /// The tile's source layer.
    pub fn source_layer(&self) -> Result<TileLayer<'a>, Error> {
        self.source_layer.layer()
    }

    /// Binders of type `P` for each layer of the group, of its evaluated
    /// paint values at the tile's zoom.
    #[must_use]
    pub fn binders<P>(&self) -> LayerBinders<P>
    where
        P: PaintBinders<Paint = <T::PaintProperties as PaintSet>::Evaluated>,
    {
        LayerBinders::new(self.layers, self.zoom())
    }

    /// Writes the values of `feature`, the feature at `index`, into
    /// `binders` for the vertices up to `vertices`, those added since the
    /// last call. `section` is the formatted section of the feature's text
    /// that the vertices draw, which a property's `["get", ..]` of the
    /// section reads; `patterns` the feature's patterns by layer, which the
    /// values take where the resolved patterns put them. A layer whose
    /// vertices draw neither passes `None` for both.
    pub fn populate<P: PaintBinders>(
        &self,
        binders: &mut LayerBinders<P>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        vertices: usize,
        section: Option<&Value>,
        patterns: Option<&PatternLayerMap>,
    ) {
        static NO_SECTION: Value = Value::Null;
        let no_patterns = ImagePositions::default();
        let positions = if patterns.is_some() { self.pattern_positions() } else { &no_patterns };
        let section = section.unwrap_or(&NO_SECTION);
        binders.populate(feature, vertices, index, positions, patterns, self.canonical(), section);
    }

    /// Adds `geometry`, the geometry of the feature at `index`, to the
    /// tile's feature index, which queries search. A layer type that places
    /// its features leaves them out: queries find what placement shows.
    pub fn index(&mut self, index: usize, geometry: &GeometryCollection) {
        let leader = &self.layers[0].style;
        self.feature_index.insert(geometry, index, &leader.source_layer, &leader.id);
    }
}

/// The group's layers by id, the first of each id kept.
pub fn layers_by_id<T: StyleLayerType>(group: &[Arc<EvaluatedLayer<T>>]) -> VecMap<String, Arc<EvaluatedLayer<T>>> {
    let mut layers = VecMap::new();
    for layer in group {
        layers.entry(layer.style.id.clone()).or_insert_with(|| Arc::clone(layer));
    }
    layers
}
