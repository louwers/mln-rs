use mln_render::layer::{Bucket, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload};
use mln_util::Color;

use super::properties::paint;

mln_render::paint_property_binders!(
    pub(crate) struct CircleBinders for paint::PossiblyEvaluated {
        circle_blur: PaintPropertyBinder<f32> => ["blur"],
        circle_color: PaintPropertyBinder<Color> => ["color"],
        circle_opacity: PaintPropertyBinder<f32> => ["opacity"],
        circle_radius: PaintPropertyBinder<f32> => ["radius"],
        circle_stroke_color: PaintPropertyBinder<Color> => ["stroke_color"],
        circle_stroke_opacity: PaintPropertyBinder<f32> => ["stroke_opacity"],
        circle_stroke_width: PaintPropertyBinder<f32> => ["stroke_width"],
    }
);

/// A tile's circles: a quad per point, each corner the point doubled with
/// the corner's right and top sides in the low bits.
#[derive(Clone, mln_wire::Wire)]
pub struct CircleBucket {
    pub(crate) corners: GpuVec<[i16; 2]>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<CircleBinders>,
}

impl Bucket for CircleBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}
