// As in circle_layout.rs, the layout keeps the indices of the features its
// filter accepted and gets the features again from the source layer in
// `create_bucket`.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::expression::EvaluationContext;
use mln_style::layers::Heatmap;
use mln_style_types::image::ImagePositions;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::error::Error;
use mln_vec_map::VecMap;

use crate::bucket::Bucket;
use crate::bucket_parameters::BucketParameters;
use crate::buckets::heatmap_bucket::HeatmapBucket;
use crate::geometry::feature_index::FeatureIndex;
use crate::layer_type::EvaluatedLayer;
use crate::layout::filter_groups::FilterGroups;
use crate::layout::layout::{BuildParams, Layout, SourceLayer, layers_by_id};
use crate::layout::pattern_layout::PatternLayerMap;
use crate::layout::tile_layout::Resolved;

pub(crate) struct HeatmapLayout {
    layers: VecMap<String, Arc<EvaluatedLayer<Heatmap>>>,
    bucket_leader_id: String,
    source_layer_id: String,
    /// The indices of the features the filter accepted, in order.
    features: Vec<usize>,
    /// The number of features in the source layer.
    feature_count: usize,
    zoom: f32,
    mode: MapMode,
    retain_features_by_id: bool,
    source_layer: SourceLayer,
}

impl HeatmapLayout {
    /// Creates the layout of `group`, layers of one layout, on
    /// `source_layer`.
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
            feature_count,
            zoom,
            mode: parameters.mode,
            retain_features_by_id: parameters.retain_features_by_id,
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
        feature_index: &mut FeatureIndex,
        build: &BuildParams,
    ) -> Result<Option<HeatmapBucket>, Error> {
        let source_layer = self.source_layer.layer()?;
        let mut bucket = HeatmapBucket::new(&self.layers, self.mode, self.zoom);
        bucket.set_retain_features_by_id(self.retain_features_by_id);
        bucket.reserve_features(self.feature_count);

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
            )?;
            feature_index.insert(&geometries, i, &self.source_layer_id, &self.bucket_leader_id);
        }
        Ok(Some(bucket))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<Heatmap>>> {
        &self.layers
    }
}
