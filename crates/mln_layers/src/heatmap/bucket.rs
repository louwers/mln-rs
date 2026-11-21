// As in the circle bucket, the vertices and indices are `GeometryVec`s, which
// a worker fills and the bucket's upload moves into the context's stores,
// which its draws read.

use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE, ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE};
use mln_map_types::MapMode;
use mln_renderer::layer::{
    Bucket, BucketBase, EvaluatedLayer, GeometryUpload, GeometryVec, Indexed, PaintPropertyBinder,
};
use mln_style::expression::Value;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::constants::EXTENT;
use mln_vec_map::VecMap;

use super::Heatmap;
use super::properties::HeatmapPaintProperties;
use crate::mesh::add_point_quads;

mln_renderer::paint_property_binders!(
    pub(crate) struct HeatmapBinders for HeatmapPaintProperties::PossiblyEvaluated {
        heatmap_radius: PaintPropertyBinder<f32> => [ID_HEATMAP_RADIUS_VERTEX_ATTRIBUTE],
        heatmap_weight: PaintPropertyBinder<f32> => [ID_HEATMAP_WEIGHT_VERTEX_ATTRIBUTE],
    }
);

/// A heatmap layout vertex, also used for the heatmap texture's quad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct HeatmapLayoutVertex {
    pub(crate) a1: [i16; 2],
}

#[derive(Clone, mln_wire::Wire)]
pub struct HeatmapBucket {
    pub(crate) base: BucketBase,
    pub(crate) vertices: GeometryVec<HeatmapLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: VecMap<String, HeatmapBinders>,
    pub(crate) mode: MapMode,
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

impl HeatmapBucket {
    #[must_use]
    pub(crate) fn new(layers: &VecMap<String, Arc<EvaluatedLayer<Heatmap>>>, mode: MapMode, zoom: f32) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, layer) in layers {
            paint_property_binders.entry(id.clone()).or_insert_with(|| HeatmapBinders::new(&layer.paint, zoom));
        }
        HeatmapBucket {
            base: BucketBase::default(),
            vertices: GeometryVec::default(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            paint_property_binders,
            mode,
        }
    }

    /// Adds the points of `feature`, the feature at `feature_index` of its
    /// source layer, with `geometry`, in the tile `canonical`.
    pub(crate) fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        feature_index: usize,
        canonical: &CanonicalTileID,
    ) {
        // Points outside the tile are left to the tiles they're in.
        let in_tile = |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
        let points = geometry.iter().flatten().filter(in_tile).copied();
        let mut triangles = Indexed { indexes: self.triangles.get_mut(), segments: &mut self.segments };
        add_point_quads(self.vertices.get_mut(), &mut triangles, points, |a1| HeatmapLayoutVertex { a1 });

        let length = self.vertices.len();
        for binders in self.paint_property_binders.values_mut() {
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

impl Bucket for HeatmapBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn update(
        &mut self,
        states: &mln_renderer::layer::FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) -> bool {
        let Some(binders) = self.paint_property_binders.get_mut(layer_id) else {
            return false;
        };
        binders.update_vertex_vectors(states, layer, image_positions);
        self.base.set_uploaded(false);
        true
    }

    fn upload(&mut self, geometry: &mut GeometryUpload<'_>) {
        geometry.move_vertices(&mut self.vertices);
        geometry.move_indexes(&mut self.triangles);
        for binders in self.paint_property_binders.values_mut() {
            let depends_on_feature_state = binders.depends_on_feature_state();
            geometry.move_paint(&mut binders.interleaved_vertex_buffer, depends_on_feature_state);
        }
        self.base.set_uploaded(true);
    }

    fn has_data(&self) -> bool {
        !self.segments.is_empty()
    }
}
