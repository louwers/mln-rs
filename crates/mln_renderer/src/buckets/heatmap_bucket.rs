// As in circle_bucket.rs, the vertices and indices are `GeometryVec`s, which
// a worker fills and the bucket's upload moves into the context's stores,
// which its draws read.

use std::sync::Arc;

use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::MapMode;
use mln_style::expression::Value;
use mln_style::layers::Heatmap;
use mln_style::layers::heatmap_layer_properties::HeatmapPaintProperties;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinate, GeometryTileFeature};
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_vec_map::VecMap;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::layer_type::EvaluatedLayer;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::paint_property_binder::PaintPropertyBinder;
use crate::util::triangulate::Triangulator;

crate::paint_property_binders!(
    pub struct HeatmapBinders for HeatmapPaintProperties::PossiblyEvaluated {
        heatmap_radius => PaintPropertyBinder<f32>,
        heatmap_weight => PaintPropertyBinder<f32>,
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

    /// The layout vertex for `p` with extrusion (`ex`, `ey`): `p.x * 2` is
    /// an integer, added to a float and truncated to `i16`.
    #[must_use]
    pub(crate) fn vertex(p: GeometryCoordinate, ex: f32, ey: f32) -> HeatmapLayoutVertex {
        HeatmapLayoutVertex {
            a1: [
                ((i32::from(p.x) * 2) as f32 + f32::midpoint(ex, 1.0)) as i32 as i16,
                ((i32::from(p.y) * 2) as f32 + f32::midpoint(ey, 1.0)) as i32 as i16,
            ],
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
        _triangulator: &mut Triangulator,
    ) -> Result<(), Error> {
        const VERTEX_LENGTH: usize = 4;

        for points in geometry.iter() {
            for &point in points {
                let x = point.x;
                let y = point.y;

                // Do not include points that are outside the tile boundaries.
                if x < 0 || i32::from(x) >= EXTENT || y < 0 || i32::from(y) >= EXTENT {
                    continue;
                }

                let (vertex_offset, index_offset) = (self.vertices.len(), self.triangles.len());
                let segment = match self.segments.last_mut() {
                    Some(segment) if u16::try_from(segment.vertex_length + VERTEX_LENGTH).is_ok() => segment,
                    // Move to a new segment because the old one can't hold
                    // the geometry.
                    _ => self.segments.push_mut(SegmentBase::new(vertex_offset, index_offset)),
                };

                // this geometry will be of the Point type, and we'll derive
                // two triangles from it.
                //
                // ┌─────────┐
                // │ 4     3 │
                // │         │
                // │ 1     2 │
                // └─────────┘
                //
                self.vertices.get_mut().push(HeatmapBucket::vertex(point, -1.0, -1.0)); // 1
                self.vertices.get_mut().push(HeatmapBucket::vertex(point, 1.0, -1.0)); // 2
                self.vertices.get_mut().push(HeatmapBucket::vertex(point, 1.0, 1.0)); // 3
                self.vertices.get_mut().push(HeatmapBucket::vertex(point, -1.0, 1.0)); // 4

                debug_assert!(u16::try_from(segment.vertex_length).is_ok());
                let index = segment.vertex_length as u16;

                // 1, 2, 3
                // 1, 4, 3
                self.triangles.get_mut().extend_from_slice(&[index, index + 1, index + 2]);
                self.triangles.get_mut().extend_from_slice(&[index, index + 3, index + 2]);

                segment.vertex_length += VERTEX_LENGTH;
                segment.index_length += 6;
            }
        }

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
        Ok(())
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
