// A pattern layout lays out the features of a layer type whose spec
// (`PatternLayoutSpec`) says how it reads patterns and sort keys; the fill
// and line specs are below. A layout that sorts its features by key, which
// the spec's `sorted` asks for, keeps them in key order as it accepts them.
//
// A `TileLayer` borrows its tile data, so the layout keeps the
// feature *indices* it accepted (`PatternFeature::i`) and gets the features
// again when it builds the bucket.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::expression::{EvaluationContext, Image};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::hash::HashMap;
use mln_style::layers::fill_layer_properties::FillLayoutProperties;
use mln_style::layers::line_layer_properties::LineLayoutProperties;
use mln_style::layers::{Fill, Line};
use mln_style::property::evaluated::Evaluated;
use mln_style::tile_id::CanonicalTileID;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};

use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::line_bucket::LineBucket;
use crate::image::image_store::ImageType;
use crate::layer::bucket::Bucket;
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::layout::{BuildCx, Layout, LayoutCx, layers_by_id};
use crate::util::tessellator::Tessellator;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PatternDependency {
    pub(crate) min: String,
    pub(crate) mid: String,
    pub(crate) max: String,
}

pub(crate) type PatternLayerMap = HashMap<String, PatternDependency>;

/// A feature the filter accepted. The feature itself is read from the source
/// layer again (see the file comment).
#[derive(Clone, Debug)]
pub(crate) struct PatternFeature {
    pub(crate) i: usize,
    pub(crate) sort_key: f32,
    patterns: PatternLayerMap,
}

impl PatternFeature {
    #[must_use]
    pub(crate) fn patterns(&self) -> &PatternLayerMap {
        &self.patterns
    }
}

/// The template arguments of a `PatternLayout` instantiation.
pub(crate) trait PatternLayoutSpec: Send + Sized + 'static {
    /// The layer type the layout lays out.
    type Type: LayerType;
    type Bucket: Bucket;
    type Layout: Clone + Send + Sync;

    fn evaluate_layout(layer: &EvaluatedLayer<Self::Type>, parameters: &PropertyEvaluationParameters) -> Self::Layout;
    fn pattern_property(layer: &EvaluatedLayer<Self::Type>) -> &Evaluated<Image>;
    /// Whether the layout of a group led by `leader` sorts its features by
    /// key (see the file comment).
    fn sorted(leader: &EvaluatedLayer<Self::Type>) -> bool;
    fn sort_key(
        layout: &Self::Layout,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        canonical: &CanonicalTileID,
    ) -> f32;
    /// The sort key when it is the same for every feature.
    fn constant_sort_key(layout: &Self::Layout) -> Option<f32>;
    /// The bucket of the layout, with no features yet.
    fn create_bucket(layout: &Self::Layout, cx: &BuildCx<'_, Self::Type>) -> Self::Bucket;
    /// Adds `feature`, the feature at `index`, with its `geometry` and the
    /// patterns of its layers, to `bucket`.
    fn add_feature(
        bucket: &mut Self::Bucket,
        cx: &BuildCx<'_, Self::Type>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        patterns: &PatternLayerMap,
        tessellator: &mut Tessellator,
    );
    /// Whether `bucket` has anything to draw.
    fn has_data(bucket: &Self::Bucket) -> bool;
    /// Records whether the layer `layer_id` draws a pattern that is an SDF
    /// image. The default does nothing.
    fn record_sdf_pattern(_bucket: &mut Self::Bucket, _layer_id: &str, _sdf: bool) {}
}

pub(crate) struct PatternLayout<S: PatternLayoutSpec> {
    pub(crate) features: Vec<PatternFeature>,
    pub(crate) layout: S::Layout,
}

impl<S: PatternLayoutSpec> Layout<S::Type> for PatternLayout<S> {
    type Bucket = S::Bucket;

    /// Whether a layer of the group has a pattern.
    fn needs_resources(group: &[Arc<EvaluatedLayer<S::Type>>]) -> bool {
        uses_pattern::<S>(group)
    }

    /// Selects the features and records the patterns they need.
    fn new(cx: &mut LayoutCx<'_, S::Type>) -> Result<Self, Error> {
        let group = cx.layers();
        let sorted = S::sorted(&group[0]);
        let tile_layer = cx.source_layer()?;
        let available_images = cx.available_images();
        let unavailable_images = cx.unavailable_images();
        let zoom = cx.zoom();
        let canonical = cx.tile().canonical;
        let layout = S::evaluate_layout(&group[0], &cx.parameters());

        let has_pattern = uses_pattern::<S>(group);
        for layer_properties in group {
            let pattern_property = S::pattern_property(layer_properties);
            let constant_pattern = pattern_property.faded();
            // Add the constant pattern dependencies.
            if pattern_property.is_constant() && !constant_pattern.to.id().is_empty() {
                cx.need_image(constant_pattern.to.id(), ImageType::Pattern);
                cx.need_image(constant_pattern.from.id(), ImageType::Pattern);
            }
        }
        let layer_properties_map = layers_by_id(group);

        let constant_sort_key = if sorted { S::constant_sort_key(&layout) } else { None };
        let mut features: Vec<PatternFeature> = Vec::new();
        let context = cx.filter_context();
        let selection = cx.select(&tile_layer, context);
        for i in 0..tile_layer.len() {
            if !selection.may_select(i) {
                continue;
            }
            let feature = tile_layer.feature(i)?;
            if !selection.selects(&feature) {
                continue;
            }
            let mut pattern_dependency_map = PatternLayerMap::default();
            if has_pattern {
                for layer_properties in group {
                    let layer_id = &layer_properties.style.id;
                    if let Some(properties) = layer_properties_map.get(layer_id) {
                        let pattern_property = S::pattern_property(properties);
                        if !pattern_property.is_constant() {
                            // For layers with non-data-constant pattern
                            // properties, evaluate their expression and add
                            // the patterns to the dependency vector
                            let at = |zoom| {
                                pattern_property.evaluate_faded(
                                    &EvaluationContext::with_zoom_feature(zoom, Some(&feature))
                                        .with_available_images(Some(available_images))
                                        .with_unavailable_images(Some(unavailable_images))
                                        .with_canonical_tile_id(Some(&canonical)),
                                )
                            };
                            let [min, mid, max] = [at(zoom - 1.0), at(zoom), at(zoom + 1.0)];

                            // An empty pattern name means no pattern, so it is not an
                            // image dependency.
                            for pattern in [&min, &mid, &max] {
                                if !pattern.to.empty() {
                                    cx.need_image(pattern.to.id(), ImageType::Pattern);
                                }
                            }
                            pattern_dependency_map.entry(layer_id.clone()).or_insert(PatternDependency {
                                min: min.to.id().to_owned(),
                                mid: mid.to.id().to_owned(),
                                max: max.to.id().to_owned(),
                            });
                        }
                    }
                }
            }

            if let Some(sort_key) = constant_sort_key {
                features.push(PatternFeature { i, sort_key, patterns: pattern_dependency_map });
            } else if sorted {
                let sort_key = S::sort_key(&layout, &feature, zoom, &canonical);
                let pattern_feature = PatternFeature { i, sort_key, patterns: pattern_dependency_map };
                // Before the first feature whose key is not less.
                let lower_bound = features.partition_point(|f| f.sort_key < sort_key);
                features.insert(lower_bound, pattern_feature);
            } else {
                features.push(PatternFeature { i, sort_key: 0.0, patterns: pattern_dependency_map });
            }
        }
        if constant_sort_key.is_some() {
            // Inserting at the lower bound puts each feature before the
            // earlier ones with an equal key: keep that order without a sort.
            features.reverse();
        }

        Ok(PatternLayout { features, layout })
    }

    fn build(self, cx: &mut BuildCx<'_, S::Type>) -> Result<Option<S::Bucket>, Error> {
        let pattern_positions = cx.pattern_positions();
        let source_layer = cx.source_layer()?;
        let mut bucket = S::create_bucket(&self.layout, cx);

        let record_sdf_pattern = |bucket: &mut S::Bucket, layer_id: &str, image_id: &str| {
            if let Some(position) = pattern_positions.get(image_id) {
                S::record_sdf_pattern(bucket, layer_id, position.sdf);
            }
        };

        for (layer_id, layer_properties) in &cx.layers_by_id() {
            let pattern_property = S::pattern_property(layer_properties);
            if pattern_property.is_constant() {
                let pattern = pattern_property.faded();
                record_sdf_pattern(&mut bucket, layer_id, pattern.from.id());
                record_sdf_pattern(&mut bucket, layer_id, pattern.to.id());
            }
        }

        let mut tessellator = Tessellator::default();
        for pattern_feature in &self.features {
            let i = pattern_feature.i;
            let feature = source_layer.feature(i)?;
            let patterns = pattern_feature.patterns();
            let geometries = feature.geometry()?;

            for (layer_id, pattern) in patterns {
                record_sdf_pattern(&mut bucket, layer_id, &pattern.min);
                record_sdf_pattern(&mut bucket, layer_id, &pattern.mid);
                record_sdf_pattern(&mut bucket, layer_id, &pattern.max);
            }

            cx.index(i, &geometries);
            S::add_feature(&mut bucket, cx, i, &feature, &geometries, patterns, &mut tessellator);
        }
        Ok(S::has_data(&bucket).then_some(bucket))
    }
}

pub(crate) struct FillLayoutSpec;

impl PatternLayoutSpec for FillLayoutSpec {
    type Type = Fill;
    type Bucket = FillBucket;
    type Layout = FillLayoutProperties::PossiblyEvaluated;

    fn evaluate_layout(layer: &EvaluatedLayer<Fill>, parameters: &PropertyEvaluationParameters) -> Self::Layout {
        layer.style.layout.evaluate(parameters)
    }
    fn pattern_property(layer: &EvaluatedLayer<Fill>) -> &Evaluated<Image> {
        &layer.paint.fill_pattern
    }
    /// Whether the leader's unevaluated layout sets `fill-sort-key`.
    fn sorted(leader: &EvaluatedLayer<Fill>) -> bool {
        !leader.style.layout.fill_sort_key.is_undefined()
    }
    fn sort_key(
        layout: &Self::Layout,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        canonical: &CanonicalTileID,
    ) -> f32 {
        layout.fill_sort_key.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
        )
    }
    fn constant_sort_key(layout: &Self::Layout) -> Option<f32> {
        layout.fill_sort_key.constant()
    }
    fn create_bucket(_layout: &Self::Layout, cx: &BuildCx<'_, Fill>) -> FillBucket {
        FillBucket::new(cx.binders(), cx.subdivision_granularity())
    }
    fn add_feature(
        bucket: &mut FillBucket,
        cx: &BuildCx<'_, Fill>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        patterns: &PatternLayerMap,
        tessellator: &mut Tessellator,
    ) {
        bucket.add_geometry(geometry, cx.canonical(), tessellator);
        let length = bucket.vertices.len();
        cx.populate(&mut bucket.paint_property_binders, index, feature, length, None, Some(patterns));
    }
    fn has_data(bucket: &FillBucket) -> bool {
        bucket.has_data()
    }
    fn record_sdf_pattern(bucket: &mut FillBucket, layer_id: &str, sdf: bool) {
        bucket.record_sdf_pattern(layer_id, sdf);
    }
}

pub(crate) struct LineLayoutSpec;

impl PatternLayoutSpec for LineLayoutSpec {
    type Type = Line;
    type Bucket = LineBucket;
    type Layout = LineLayoutProperties::PossiblyEvaluated;

    fn evaluate_layout(layer: &EvaluatedLayer<Line>, parameters: &PropertyEvaluationParameters) -> Self::Layout {
        layer.style.layout.evaluate(parameters)
    }
    fn pattern_property(layer: &EvaluatedLayer<Line>) -> &Evaluated<Image> {
        &layer.paint.line_pattern
    }
    /// Whether the leader's unevaluated layout sets `line-sort-key`.
    fn sorted(leader: &EvaluatedLayer<Line>) -> bool {
        !leader.style.layout.line_sort_key.is_undefined()
    }
    fn sort_key(
        layout: &Self::Layout,
        feature: &dyn GeometryTileFeature,
        zoom: f32,
        canonical: &CanonicalTileID,
    ) -> f32 {
        layout.line_sort_key.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
        )
    }
    fn constant_sort_key(layout: &Self::Layout) -> Option<f32> {
        layout.line_sort_key.constant()
    }
    fn create_bucket(layout: &Self::Layout, cx: &BuildCx<'_, Line>) -> LineBucket {
        LineBucket::new(layout.clone(), cx.binders(), cx.tile(), cx.subdivision_granularity())
    }
    fn add_feature(
        bucket: &mut LineBucket,
        cx: &BuildCx<'_, Line>,
        index: usize,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        patterns: &PatternLayerMap,
        tessellator: &mut Tessellator,
    ) {
        for line in geometry.iter() {
            bucket.add_geometry(line, feature, cx.canonical(), tessellator);
        }
        let length = bucket.vertices.len();
        cx.populate(&mut bucket.paint_property_binders, index, feature, length, None, Some(patterns));
    }
    fn has_data(bucket: &LineBucket) -> bool {
        bucket.has_data()
    }
}

/// Whether a layer of `group` uses its `*-pattern` property, so that the
/// group's layout needs images.
pub(crate) fn uses_pattern<S: PatternLayoutSpec>(group: &[Arc<EvaluatedLayer<S::Type>>]) -> bool {
    group.iter().any(|layer_properties| {
        let pattern_property = S::pattern_property(layer_properties);
        !pattern_property.is_constant() || !pattern_property.faded().to.id().is_empty()
    })
}
