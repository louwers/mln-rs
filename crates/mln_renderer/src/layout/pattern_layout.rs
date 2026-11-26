// A pattern layout lays out the features of a layer type whose spec
// (`PatternLayoutSpec`) says how it reads patterns and sort keys; the fill
// and line specs are below. A layout that sorts its features by key, which
// the spec's `sorted` asks for, keeps them in key order as it accepts them.
//
// A `TileLayer` borrows its tile data, so the layout keeps the
// feature *indices* it accepted (`PatternFeature::i`) and gets the features
// again in `create_bucket`.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::expression::{EvaluationContext, Image};
use mln_style::feature::GeometryTileFeature;
use mln_style::hash::HashMap;
use mln_style::layers::fill_layer_properties::FillLayoutProperties;
use mln_style::layers::line_layer_properties::LineLayoutProperties;
use mln_style::layers::{Fill, Line};
use mln_style::property::evaluated::Evaluated;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_text::atlas::ImagePositions;
use mln_vec_map::VecMap;

use crate::bucket::Bucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::line_bucket::LineBucket;
use crate::geometry::feature_index::FeatureIndexBuilder;
use crate::image_store::ImageType;
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{BuildParams, Layout, LayoutParameters, SourceLayer, layers_by_id};
use crate::layout::tile_layout::Resolved;
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
pub(crate) trait PatternLayoutSpec: Send + 'static {
    /// The layer type the layout lays out.
    type Type: LayerType<Bucket = Self::Bucket>;
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
    fn create_bucket(
        layout: &Self::Layout,
        layer_properties_map: &VecMap<String, Arc<EvaluatedLayer<Self::Type>>>,
        zoom: f32,
        overscaling: u32,
    ) -> Self::Bucket;
    /// Sets the subdivision granularity on the buckets that have one. The
    /// default does nothing.
    fn set_subdivision_granularity(_bucket: &mut Self::Bucket, _value: &SubdivisionGranularitySetting) {}
}

pub(crate) struct PatternLayout<S: PatternLayoutSpec> {
    pub(crate) layer_properties_map: VecMap<String, Arc<EvaluatedLayer<S::Type>>>,
    pub(crate) bucket_leader_id: String,

    pub(crate) features: Vec<PatternFeature>,
    pub(crate) layout: S::Layout,

    pub(crate) zoom: f32,
    pub(crate) overscaling: u32,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
    pub(crate) source_layer_id: String,
    pub(crate) has_pattern: bool,

    /// The source layer the layout gets its features from again (see
    /// layout.rs).
    pub(crate) source_layer: SourceLayer,
}

impl<S: PatternLayoutSpec> PatternLayout<S> {
    /// Creates the layout of `group`, layers of one layout, on
    /// `source_layer`, and records the patterns it needs in
    /// `layout_parameters`.
    pub(crate) fn new(
        layout_parameters: &mut LayoutParameters<'_>,
        group: &[Arc<EvaluatedLayer<S::Type>>],
        source_layer: SourceLayer,
    ) -> Result<Self, Error> {
        debug_assert!(!group.is_empty());
        let sorted = S::sorted(&group[0]);
        let tile_layer = source_layer.layer()?;
        let parameters = layout_parameters.bucket_parameters;
        let image_dependencies = &mut *layout_parameters.image_dependencies;
        let available_images = layout_parameters.available_images;
        let unavailable_images = layout_parameters.unavailable_images;
        let zoom = f32::from(parameters.tile_id.overscaled_z);
        let overscaling = parameters.tile_id.overscale_factor();
        let leader_layer_properties = &group[0];
        let mut evaluation_parameters = PropertyEvaluationParameters::new(zoom);
        evaluation_parameters.global_state = parameters.global_state.clone();
        let layout = S::evaluate_layout(leader_layer_properties, &evaluation_parameters);
        let leader_impl = &leader_layer_properties.style;
        let source_layer_id = leader_impl.source_layer.clone();
        let bucket_leader_id = leader_impl.id.clone();

        let has_pattern = uses_pattern::<S>(group);
        for layer_properties in group {
            let pattern_property = S::pattern_property(layer_properties);
            let constant_pattern = pattern_property.faded();
            // Add the constant pattern dependencies.
            if pattern_property.is_constant() && !constant_pattern.to.id().is_empty() {
                image_dependencies.entry(constant_pattern.to.id().to_owned()).or_insert(ImageType::Pattern);
                image_dependencies.entry(constant_pattern.from.id().to_owned()).or_insert(ImageType::Pattern);
            }
        }
        let layer_properties_map = layers_by_id(group);

        let constant_sort_key = if sorted { S::constant_sort_key(&layout) } else { None };
        let mut features: Vec<PatternFeature> = Vec::new();
        let feature_count = tile_layer.len();
        let context = EvaluationContext::new(zoom)
            .with_canonical_tile_id(Some(&parameters.tile_id.canonical))
            .with_global_state(parameters.global_state.as_deref());
        let passes = |feature: &dyn GeometryTileFeature| {
            leader_impl.filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
        };
        let selected = layout_parameters.filter_groups.select(
            &source_layer_id,
            &tile_layer,
            &leader_impl.filter,
            &context,
            passes,
        );
        for i in 0..feature_count {
            if selected.as_ref().is_some_and(|selected| !selected[i]) {
                continue;
            }
            let feature = tile_layer.feature(i)?;
            if selected.is_none() && !passes(&feature) {
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
                            let canonical = &parameters.tile_id.canonical;
                            let min = pattern_property.evaluate_faded(
                                &EvaluationContext::with_zoom_feature(zoom - 1.0, Some(&feature))
                                    .with_available_images(Some(available_images))
                                    .with_unavailable_images(Some(unavailable_images))
                                    .with_canonical_tile_id(Some(canonical)),
                            );
                            let mid = pattern_property.evaluate_faded(
                                &EvaluationContext::with_zoom_feature(zoom, Some(&feature))
                                    .with_available_images(Some(available_images))
                                    .with_unavailable_images(Some(unavailable_images))
                                    .with_canonical_tile_id(Some(canonical)),
                            );
                            let max = pattern_property.evaluate_faded(
                                &EvaluationContext::with_zoom_feature(zoom + 1.0, Some(&feature))
                                    .with_available_images(Some(available_images))
                                    .with_unavailable_images(Some(unavailable_images))
                                    .with_canonical_tile_id(Some(canonical)),
                            );

                            // An empty pattern name means no pattern, so it is not an
                            // image dependency.
                            for pattern in [&min, &mid, &max] {
                                if !pattern.to.empty() {
                                    image_dependencies.entry(pattern.to.id().to_owned()).or_insert(ImageType::Pattern);
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
                let sort_key = S::sort_key(&layout, &feature, zoom, &parameters.tile_id.canonical);
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

        Ok(PatternLayout {
            layer_properties_map,
            bucket_leader_id,
            features,
            layout,
            zoom,
            overscaling,
            subdivision_granularity: parameters.subdivision_granularity,
            source_layer_id,
            has_pattern,
            source_layer,
        })
    }
}

impl<S: PatternLayoutSpec> Layout<S::Type> for PatternLayout<S> {
    fn has_dependencies(&self) -> bool {
        self.has_pattern
    }

    fn create_bucket(
        &mut self,
        resolved: Option<&Resolved>,
        feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<S::Bucket>, Error> {
        let no_patterns = ImagePositions::default();
        let pattern_positions = resolved.map_or(&no_patterns, |resolved| &resolved.patterns);
        let source_layer = self.source_layer.layer()?;
        let mut bucket = S::create_bucket(&self.layout, &self.layer_properties_map, self.zoom, self.overscaling);
        S::set_subdivision_granularity(&mut bucket, &self.subdivision_granularity);

        let record_sdf_pattern = |bucket: &mut S::Bucket, layer_id: &str, image_id: &str| {
            if let Some(position) = pattern_positions.get(image_id) {
                bucket.record_sdf_pattern(layer_id, position.sdf);
            }
        };

        for (layer_id, layer_properties) in &self.layer_properties_map {
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

            feature_index.insert(&geometries, i, &self.source_layer_id, &self.bucket_leader_id);
            bucket.add_feature(
                &feature,
                &geometries,
                pattern_positions,
                patterns,
                i,
                &build.canonical,
                &mut tessellator,
            );
        }
        Ok(Some(bucket))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<S::Type>>> {
        &self.layer_properties_map
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
    fn create_bucket(
        layout: &Self::Layout,
        layer_properties_map: &VecMap<String, Arc<EvaluatedLayer<Fill>>>,
        zoom: f32,
        overscaling: u32,
    ) -> FillBucket {
        FillBucket::new(layout, layer_properties_map, zoom, overscaling)
    }
    fn set_subdivision_granularity(bucket: &mut FillBucket, value: &SubdivisionGranularitySetting) {
        bucket.set_subdivision_granularity(value);
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
    fn create_bucket(
        layout: &Self::Layout,
        layer_properties_map: &VecMap<String, Arc<EvaluatedLayer<Line>>>,
        zoom: f32,
        overscaling: u32,
    ) -> LineBucket {
        LineBucket::new(layout.clone(), layer_properties_map, zoom, overscaling)
    }
    fn set_subdivision_granularity(bucket: &mut LineBucket, value: &SubdivisionGranularitySetting) {
        bucket.set_subdivision_granularity(value);
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
