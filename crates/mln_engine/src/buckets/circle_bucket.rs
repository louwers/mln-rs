// The vertices and indices are `GeometryVec`s, which a worker fills and the
// bucket's upload moves into the context's stores, which its draws read.

use std::sync::Arc;

use mln_style::color::Color;
use mln_style::layers::Circle;
use mln_style::layers::circle_layer_properties::CirclePaintProperties;
use mln_vec_map::VecMap;

use crate::draw::segment::SegmentBase;
use crate::layer::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::layer::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::PaintPropertyBinder;
use crate::map::MapMode;
use crate::render::terrain::point_center;
use crate::text::atlas::ImagePositions;

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

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct CircleBucket {
    pub(crate) base: BucketBase,
    pub(crate) vertices: GeometryVec<CircleLayoutVertex>,
    /// The center of each vertex's point, which the upload keeps before the
    /// vertices move: where the heights of the ground under them are.
    #[wire(skip)]
    pub(crate) centers: Vec<[i16; 2]>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: VecMap<String, CircleBinders>,
    pub(crate) mode: MapMode,
}

impl CircleBucket {
    #[must_use]
    pub(crate) fn new(
        layer_paint_properties: &VecMap<String, Arc<EvaluatedLayer<Circle>>>,
        mode: MapMode,
        zoom: f32,
    ) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, properties) in layer_paint_properties {
            paint_property_binders.entry(id.clone()).or_insert_with(|| CircleBinders::new(&properties.paint, zoom));
        }
        CircleBucket {
            base: BucketBase::default(),
            vertices: GeometryVec::default(),
            centers: Vec::new(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            paint_property_binders,
            mode,
        }
    }

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
    fn base(&self) -> &BucketBase {
        &self.base
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

    fn update(
        &mut self,
        states: &crate::layer::bucket::FeatureStates,
        layer: &crate::tile::TileLayer<'_>,
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
}
