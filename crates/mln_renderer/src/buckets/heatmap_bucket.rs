// As in circle_bucket.rs, the vertices and indices are `GeometryVec`s, which
// a worker fills and the bucket's upload moves into the context's stores,
// which its draws read.

use std::sync::Arc;

use geometry::Point;
use mln_map_types::MapMode;
use mln_style::constants::EXTENT;
use mln_style::expression::Value;
use mln_style::feature::{GeometryCollection, GeometryCoordinate, GeometryTileFeature};
use mln_style::layers::Heatmap;
use mln_style::layers::heatmap_layer_properties::HeatmapPaintProperties;
use mln_style::tile_id::CanonicalTileID;
use mln_text::atlas::ImagePositions;
use mln_vec_map::VecMap;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::gfx::mesh::{Indexed, add_point_quads};
use crate::gfx::segment::SegmentBase;
use crate::layer_type::EvaluatedLayer;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::paint_property_binder::PaintPropertyBinder;
use crate::terrain::point_center;
use crate::util::tessellator::Tessellator;

crate::paint_property_binders!(
    pub struct HeatmapBinders for HeatmapPaintProperties::PossiblyEvaluated {
        heatmap_radius: PaintPropertyBinder<f32> => ["radius"],
        heatmap_weight: PaintPropertyBinder<f32> => ["weight"],
    }
);

/// A heatmap layout vertex, also used for the heatmap texture's quad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct HeatmapLayoutVertex {
    pub(crate) a1: [i16; 2],
}

pub(crate) type HeatmapTextureLayoutVertex = HeatmapLayoutVertex;

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct HeatmapBucket {
    pub(crate) base: BucketBase,
    pub(crate) vertices: GeometryVec<HeatmapLayoutVertex>,
    /// The center of each vertex's point, which the upload keeps before the
    /// vertices move: where the heights of the ground under them are.
    #[wire(skip)]
    pub(crate) centers: Vec<[i16; 2]>,
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
            centers: Vec::new(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            paint_property_binders,
            mode,
        }
    }

    #[must_use]
    pub(crate) fn texture_vertex(p: GeometryCoordinate) -> HeatmapTextureLayoutVertex {
        HeatmapLayoutVertex { a1: [p.x, p.y] }
    }
}

impl Bucket for HeatmapBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        _image_positions: &ImagePositions,
        _patterns: &PatternLayerMap,
        feature_index: usize,
        canonical: &CanonicalTileID,
        _tessellator: &mut Tessellator,
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

    fn update(
        &mut self,
        states: &crate::bucket::FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) -> bool {
        let Some(binders) = self.paint_property_binders.get_mut(layer_id) else {
            return false;
        };
        binders.update_vertex_vectors(states, layer, image_positions);
        self.base.uploaded = false;
        true
    }

    fn upload(&mut self, geometry: &mut GeometryUpload<'_>, _needs: UploadNeeds) {
        if !self.vertices.is_moved() {
            self.centers = self.vertices.data().iter().map(|vertex| point_center(vertex.a1)).collect();
        }
        geometry.move_vertices(&mut self.vertices);
        geometry.move_indexes(&mut self.triangles);
        for binders in self.paint_property_binders.values_mut() {
            let depends_on_feature_state = binders.depends_on_feature_state();
            geometry.move_paint(&mut binders.interleaved_vertex_buffer, depends_on_feature_state);
        }
        self.base.uploaded = true;
    }

    fn has_data(&self) -> bool {
        !self.segments.is_empty()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        mln_wire::Wire::write(self, w);
    }
}
