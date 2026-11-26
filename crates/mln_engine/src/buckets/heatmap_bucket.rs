// As in circle_bucket.rs, the vertices and indices are `GeometryVec`s, which
// a worker fills and the bucket's upload moves into the context's stores,
// which its draws read.

use geometry::Point;
use mln_style::constants::EXTENT;
use mln_style::feature::{GeometryCollection, GeometryCoordinate};
use mln_style::layers::heatmap_layer_properties::HeatmapPaintProperties;

use crate::draw::mesh::{Indexed, add_point_quads};
use crate::draw::segment::SegmentBase;
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::GeometryVec;
use crate::layer::paint_property_binder::{LayerBinders, PaintPropertyBinder};
use crate::render::terrain::point_center;

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

#[derive(Clone, Default, mln_wire::Wire)]
pub(crate) struct HeatmapBucket {
    pub(crate) vertices: GeometryVec<HeatmapLayoutVertex>,
    /// The center of each vertex's point, which the upload keeps before the
    /// vertices move: where the heights of the ground under them are.
    #[wire(skip)]
    pub(crate) centers: Vec<[i16; 2]>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: LayerBinders<HeatmapBinders>,
}

impl HeatmapBucket {
    #[must_use]
    pub(crate) fn texture_vertex(p: GeometryCoordinate) -> HeatmapTextureLayoutVertex {
        HeatmapLayoutVertex { a1: [p.x, p.y] }
    }

    /// Adds a quad for each point of `geometry` in the tile; points outside
    /// it are left to the tiles they're in.
    pub(crate) fn add_points(&mut self, geometry: &GeometryCollection) {
        let in_tile = |p: &&Point<i16>| (0..EXTENT).contains(&i32::from(p.x)) && (0..EXTENT).contains(&i32::from(p.y));
        let points = geometry.iter().flatten().filter(in_tile).copied();
        let mut triangles = Indexed { indexes: self.triangles.get_mut(), segments: &mut self.segments };
        add_point_quads(self.vertices.get_mut(), &mut triangles, points, |a1| HeatmapLayoutVertex { a1 });
    }
}

impl Bucket for HeatmapBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        if up.caps().is_some() && !self.vertices.is_moved() {
            self.centers = self.vertices.data().iter().map(|vertex| point_center(vertex.a1)).collect();
        }
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.paint_property_binders);
    }
}
