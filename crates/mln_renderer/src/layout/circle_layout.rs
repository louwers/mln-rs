// As in pattern_layout.rs, the layout keeps the indices (and sort keys) of
// the features its filter accepted and gets the features again from the
// source layer in `create_bucket`.

use std::sync::Arc;

use geometry::Point;
use mln_map_types::MapMode;
use mln_style::expression::{EvaluationContext, Value};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::layers::{Circle, CircleLayoutProperties};
use mln_style::tile_id::CanonicalTileID;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_text::atlas::ImagePositions;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_vec_map::VecMap;

use crate::bucket_parameters::BucketParameters;
use crate::buckets::circle_bucket::{CircleBucket, CircleLayoutVertex};
use crate::geometry::feature_index::FeatureIndexBuilder;
use crate::gfx::mesh::{Indexed, add_point_quads};
use crate::layer_type::EvaluatedLayer;
use crate::layout::filter_groups::FilterGroups;
use crate::layout::layout::{BuildParams, Layout, SourceLayer, layers_by_id};
use crate::layout::tile_layout::Resolved;

/// A feature the filter accepted, by index (see the file comment).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CircleFeature {
    pub(crate) i: usize,
    pub(crate) sort_key: f32,
}

pub(crate) struct CircleLayout {
    pub(crate) layer_properties_map: VecMap<String, Arc<EvaluatedLayer<Circle>>>,
    pub(crate) bucket_leader_id: String,

    pub(crate) features: Vec<CircleFeature>,

    pub(crate) zoom: f32,
    pub(crate) mode: MapMode,
    pub(crate) source_layer_id: String,

    /// The source layer the layout gets its features from again (see
    /// layout.rs).
    pub(crate) source_layer: SourceLayer,
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

impl CircleLayout {
    pub(crate) fn new(
        parameters: &BucketParameters,
        group: &[Arc<EvaluatedLayer<Circle>>],
        source_layer: SourceLayer,
        filter_groups: &mut FilterGroups,
    ) -> Result<Self, Error> {
        debug_assert!(!group.is_empty());
        let tile_layer = source_layer.layer()?;
        let zoom = f32::from(parameters.tile_id.overscaled_z);
        let leader_impl = &group[0].style;
        let unevaluated_layout = &leader_impl.layout;
        let sort_features_by_key = !unevaluated_layout.circle_sort_key.is_undefined();
        let mut evaluation_parameters = PropertyEvaluationParameters::new(zoom);
        evaluation_parameters.global_state = parameters.global_state.clone();
        let layout = unevaluated_layout.evaluate(&evaluation_parameters);
        let sort_key_property = &layout.circle_sort_key;
        let constant_sort_key = if sort_features_by_key { sort_key_property.constant() } else { None };
        let source_layer_id = leader_impl.source_layer.clone();
        let bucket_leader_id = leader_impl.id.clone();

        let layer_properties_map = layers_by_id(group);

        let mut features: Vec<CircleFeature> = Vec::new();
        let feature_count = tile_layer.len();
        let context = EvaluationContext::new(zoom)
            .with_canonical_tile_id(Some(&parameters.tile_id.canonical))
            .with_global_state(parameters.global_state.as_deref());
        let passes = |feature: &dyn GeometryTileFeature| {
            leader_impl.filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
        };
        let selected = filter_groups.select(&source_layer_id, &tile_layer, &leader_impl.filter, &context, passes);
        for i in 0..feature_count {
            if selected.as_ref().is_some_and(|selected| !selected[i]) {
                continue;
            }
            let feature = tile_layer.feature(i)?;
            if selected.is_none() && !passes(&feature) {
                continue;
            }

            if !sort_features_by_key {
                features.push(CircleFeature { i, sort_key: CircleLayoutProperties::Defaults::circle_sort_key() });
                continue;
            }

            if let Some(sort_key) = constant_sort_key {
                features.push(CircleFeature { i, sort_key });
                continue;
            }

            let sort_key = sort_key_property.evaluate(&EvaluationContext::with_zoom_feature(zoom, Some(&feature)));
            let circle_feature = CircleFeature { i, sort_key };
            // std::lower_bound: the first feature whose key is not less.
            let sort_position = features.partition_point(|f| f.sort_key < sort_key);
            features.insert(sort_position, circle_feature);
        }
        if constant_sort_key.is_some() {
            // Inserting at the lower bound puts each feature before the
            // earlier ones with an equal key: keep that order without a sort.
            features.reverse();
        }

        Ok(CircleLayout {
            layer_properties_map,
            bucket_leader_id,
            features,
            zoom,
            mode: parameters.mode,
            source_layer_id,
            source_layer,
        })
    }

    fn add_circle(
        bucket: &mut CircleBucket,
        mode: MapMode,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        feature_index: usize,
        canonical: &CanonicalTileID,
    ) {
        // A map of many tiles leaves the points outside the tile to the
        // tiles they're in, so that a circle that several tiles hold in
        // their buffers draws once. A tile image draws them all, so that
        // circles from a neighbouring tile aren't cut off at its edge.
        let in_tile = |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
        let points = geometry.iter().flatten().filter(|p| mode == MapMode::Tile || in_tile(p)).copied();
        let mut triangles = Indexed { indexes: bucket.triangles.get_mut(), segments: &mut bucket.segments };
        add_point_quads(bucket.vertices.get_mut(), &mut triangles, points, |a1| CircleLayoutVertex { a1 });

        let length = bucket.vertices.len();
        for binders in bucket.paint_property_binders.values_mut() {
            binders.populate_vertex_vectors(
                feature,
                length,
                feature_index,
                &ImagePositions::default(),
                None,
                canonical,
                &EMPTY_FORMATTED_SECTION,
            );
        }
    }
}

impl Layout<Circle> for CircleLayout {
    fn has_dependencies(&self) -> bool {
        false
    }

    fn create_bucket(
        &mut self,
        _resolved: Option<&Resolved>,
        feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<CircleBucket>, Error> {
        let source_layer = self.source_layer.layer()?;
        let mut bucket = CircleBucket::new(&self.layer_properties_map, self.mode, self.zoom);

        for circle_feature in &self.features {
            let i = circle_feature.i;
            let feature = source_layer.feature(i)?;
            let geometries = feature.geometry()?;

            CircleLayout::add_circle(&mut bucket, self.mode, &feature, &geometries, i, &build.canonical);

            feature_index.insert(&geometries, i, &self.source_layer_id, &self.bucket_leader_id);
        }

        Ok(Some(bucket))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<Circle>>> {
        &self.layer_properties_map
    }
}
