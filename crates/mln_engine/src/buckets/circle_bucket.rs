// The vertices and indices are `GeometryVec`s, which a worker fills and the
// bucket's upload moves into the context's stores, which its draws read.

use mln_style::color::Color;
use mln_style::layers::Circle;
use mln_style::layers::circle_layer_properties::CirclePaintProperties;

use crate::draw::segment::SegmentBase;
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::GeometryVec;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::{LayerBinders, PaintPropertyBinder};
use crate::render::terrain::point_center;

crate::paint_property_binders!(
    pub struct CircleBinders for CirclePaintProperties::PossiblyEvaluated {
        circle_blur: PaintPropertyBinder<f32> => ["blur"],
        circle_color: PaintPropertyBinder<Color> => ["color"],
        circle_opacity: PaintPropertyBinder<f32> => ["opacity"],
        circle_radius: PaintPropertyBinder<f32> => ["radius"],
        circle_stroke_color: PaintPropertyBinder<Color> => ["stroke_color"],
        circle_stroke_opacity: PaintPropertyBinder<f32> => ["stroke_opacity"],
        circle_stroke_width: PaintPropertyBinder<f32> => ["stroke_width"],
    }
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct CircleLayoutVertex {
    pub(crate) a1: [i16; 2],
}

#[derive(Clone, Default, mln_wire::Wire)]
pub(crate) struct CircleBucket {
    pub(crate) vertices: GeometryVec<CircleLayoutVertex>,
    /// The center of each vertex's point, which the upload keeps before the
    /// vertices move: where the heights of the ground under them are.
    #[wire(skip)]
    pub(crate) centers: Vec<[i16; 2]>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: LayerBinders<CircleBinders>,
}

impl CircleBucket {
    /// How far outside a feature a query for `layer` still hits it, from
    /// the layer's current values and the bucket's statistics of them.
    pub(crate) fn query_radius(&self, layer: &EvaluatedLayer<Circle>) -> f32 {
        let evaluated = &layer.paint;
        let binders = self.paint_property_binders.get(&layer.style.id);
        let radius = radius_or_width(
            evaluated.circle_radius.constant(),
            CirclePaintProperties::Defaults::circle_radius(),
            binders.and_then(|b| b.circle_radius.statistics.max()),
        );
        let stroke = radius_or_width(
            evaluated.circle_stroke_width.constant(),
            CirclePaintProperties::Defaults::circle_stroke_width(),
            binders.and_then(|b| b.circle_stroke_width.statistics.max()),
        );
        let translate = evaluated.circle_translate.uniform();
        // The translation's length, in `f32`.
        radius + stroke + (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }
}

/// A data-driven property's largest value over the bucket's features, from
/// its binder's `statistics_max`, or else its constant value or default.
fn radius_or_width(evaluated_constant: Option<f32>, default_value: f32, statistics_max: Option<f32>) -> f32 {
    match statistics_max {
        Some(max) => max,
        None => evaluated_constant.unwrap_or(default_value),
    }
}

impl Bucket for CircleBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        if up.caps().is_some() && !self.vertices.is_moved() {
            self.centers = self.vertices.data().iter().map(|vertex| point_center(vertex.a1)).collect();
        }
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.paint_property_binders);
    }
}
