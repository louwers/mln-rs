// As in circle_layout.rs, the layout keeps the indices of the features its
// filter accepted and gets the features again from the source layer in
// `create_bucket`.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::expression::EvaluationContext;
use mln_style::feature::GeometryTileFeature;
use mln_style::layers::Heatmap;
use mln_vec_map::VecMap;

use crate::buckets::heatmap_bucket::HeatmapBucket;
use crate::layer::bucket::Bucket;
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::filter_groups::FilterGroups;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::layout::{BuildParams, Layout, SourceLayer, layers_by_id};
use crate::layer::tile_layout::Resolved;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::map::MapMode;
use crate::query::feature_index::FeatureIndexBuilder;
use crate::text::atlas::ImagePositions;
use crate::util::tessellator::Tessellator;

pub(crate) struct HeatmapLayout {
    layers: VecMap<String, Arc<EvaluatedLayer<Heatmap>>>,
    bucket_leader_id: String,
    source_layer_id: String,
    /// The indices of the features the filter accepted, in order.
    features: Vec<usize>,
    /// The number of features in the source layer.
    zoom: f32,
    mode: MapMode,
    source_layer: SourceLayer,
}

impl HeatmapLayout {
    pub(crate) fn new(
        parameters: &BucketParameters,
        group: &[Arc<EvaluatedLayer<Heatmap>>],
        source_layer: SourceLayer,
        filter_groups: &mut FilterGroups,
    ) -> Result<Self, Error> {
        debug_assert!(!group.is_empty());
        let tile_layer = source_layer.layer()?;
        let zoom = f32::from(parameters.tile_id.overscaled_z);
        let leader = &group[0].style;
        let filter = &leader.filter;
        let context = EvaluationContext::new(zoom)
            .with_canonical_tile_id(Some(&parameters.tile_id.canonical))
            .with_global_state(parameters.global_state.as_deref());
        let passes = |feature: &dyn GeometryTileFeature| {
            filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
        };
        let selected = filter_groups.select(&leader.source_layer, &tile_layer, filter, &context, passes);

        let feature_count = tile_layer.len();
        let mut features = Vec::new();
        for i in 0..feature_count {
            if selected.as_ref().is_some_and(|selected| !selected[i]) {
                continue;
            }
            if selected.is_none() && !passes(&tile_layer.feature(i)?) {
                continue;
            }
            features.push(i);
        }

        Ok(HeatmapLayout {
            layers: layers_by_id(group),
            bucket_leader_id: leader.id.clone(),
            source_layer_id: leader.source_layer.clone(),
            features,
            zoom,
            mode: parameters.mode,
            source_layer,
        })
    }
}

impl Layout<Heatmap> for HeatmapLayout {
    fn has_dependencies(&self) -> bool {
        false
    }

    fn create_bucket(
        &mut self,
        _resolved: Option<&Resolved>,
        feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<HeatmapBucket>, Error> {
        let source_layer = self.source_layer.layer()?;
        let mut bucket = HeatmapBucket::new(&self.layers, self.mode, self.zoom);

        for &i in &self.features {
            let feature = source_layer.feature(i)?;
            let geometries = feature.geometry()?;
            bucket.add_feature(
                &feature,
                &geometries,
                &ImagePositions::default(),
                &PatternLayerMap::default(),
                i,
                &build.canonical,
                &mut Tessellator::default(),
            );
            feature_index.insert(&geometries, i, &self.source_layer_id, &self.bucket_leader_id);
        }
        Ok(Some(bucket))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<Heatmap>>> {
        &self.layers
    }
}
