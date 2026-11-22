use mln_gfx::shaders::segment::SegmentBase;
use mln_render::layer::{Bucket, GpuVec, LayerBinders, PaintPropertyBinder, Upload};
use mln_util::Color;

use super::CirclePaintProperties;

mln_render::paint_property_binders!(
    pub(crate) struct CircleBinders for CirclePaintProperties::PossiblyEvaluated {
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
    pub(crate) triangles: GpuVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) binders: LayerBinders<CircleBinders>,
}

impl Bucket for CircleBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.corners);
        up.vec(&mut self.triangles);
        up.binders(&mut self.binders);
    }
}
