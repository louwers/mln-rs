use mln_render::layer::{Bucket, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload};

use super::properties::paint;

mln_render::paint_property_binders!(
    pub(crate) struct HeatmapBinders for paint::PossiblyEvaluated {
        heatmap_radius: PaintPropertyBinder<f32> => ["radius"],
        heatmap_weight: PaintPropertyBinder<f32> => ["weight"],
    }
);

/// A tile's points: a quad each, each corner the point doubled with the
/// corner's right and top sides in the low bits.
#[derive(Clone, mln_wire::Wire)]
pub struct HeatmapBucket {
    pub(crate) corners: GpuVec<[i16; 2]>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<HeatmapBinders>,
}

impl Bucket for HeatmapBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}
