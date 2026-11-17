// The vertex/index vectors are plain `Vec`s here (the bucket is built on the
// worker thread and shared with `Arc`); the render layer copies them into
// the shared gfx vectors its drawables reference.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mln_gfx::shaders::segment::SegmentBase;
use mln_map_types::MapMode;
use mln_style::layers::Circle;
use mln_style::layers::circle_layer_properties::CirclePaintProperties;
use mln_style_types::image::ImagePositions;
use mln_util::Color;
use mln_vec_map::VecMap;
use parking_lot::RwLock;

use crate::bucket::{Bucket, BucketBase};
use crate::bucket_geometry::GeometryVec;
use crate::gfx::context::Context;
use crate::layer_type::EvaluatedLayer;
use crate::paint_property_binder::PaintPropertyBinder;

crate::paint_property_binders!(
    pub struct CircleBinders for CirclePaintProperties::PossiblyEvaluated {
        circle_blur => PaintPropertyBinder<f32>,
        circle_color => PaintPropertyBinder<Color>,
        circle_opacity => PaintPropertyBinder<f32>,
        circle_radius => PaintPropertyBinder<f32>,
        circle_stroke_color => PaintPropertyBinder<Color>,
        circle_stroke_opacity => PaintPropertyBinder<f32>,
        circle_stroke_width => PaintPropertyBinder<f32>,
    }
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct CircleLayoutVertex {
    pub(crate) a1: [i16; 2],
}

#[derive(mln_wire::Wire)]
pub(crate) struct CircleBucket {
    pub(crate) base: BucketBase,
    pub(crate) vertices: GeometryVec<CircleLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    /// Behind `RwLock`s: `update` (feature state) changes them while the
    /// bucket is shared.
    pub(crate) paint_property_binders: VecMap<String, RwLock<CircleBinders>>,
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
            paint_property_binders
                .entry(id.clone())
                .or_insert_with(|| RwLock::new(CircleBinders::new(&properties.paint, zoom)));
        }
        CircleBucket {
            base: BucketBase::default(),
            vertices: GeometryVec::default(),
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
        let radius = get_radius_or_width(
            evaluated.circle_radius.constant(),
            CirclePaintProperties::Defaults::circle_radius(),
            binders.and_then(|b| b.read().circle_radius.statistics.max()),
        );
        let stroke = get_radius_or_width(
            evaluated.circle_stroke_width.constant(),
            CirclePaintProperties::Defaults::circle_stroke_width(),
            binders.and_then(|b| b.read().circle_stroke_width.statistics.max()),
        );
        let translate = evaluated.circle_translate.uniform();
        // util::length(a, b) = std::sqrt(a * a + b * b), in float
        radius + stroke + (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }

    /// The layout vertex for `p` with extrusion (`ex`, `ey`). As in C++,
    /// `p.x * 2` is an integer, added to a float and truncated to `i16`.
    #[must_use]
    pub(crate) fn vertex(
        p: mln_tile_id::geometry_tile_data::GeometryCoordinate,
        ex: f32,
        ey: f32,
    ) -> CircleLayoutVertex {
        CircleLayoutVertex {
            a1: [
                ((i32::from(p.x) * 2) as f32 + f32::midpoint(ex, 1.0)) as i32 as i16,
                ((i32::from(p.y) * 2) as f32 + f32::midpoint(ey, 1.0)) as i32 as i16,
            ],
        }
    }
}

/// The anonymous-namespace `get<Property>(evaluated, id, binders)` of
/// `circle_bucket.cpp`.
fn get_radius_or_width(evaluated_constant: Option<f32>, default_value: f32, statistics_max: Option<f32>) -> f32 {
    match statistics_max {
        Some(max) => max,
        None => evaluated_constant.unwrap_or(default_value),
    }
}

impl Bucket for CircleBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn upload(&self, _context: &Context) {
        self.base.uploaded.store(true, Ordering::SeqCst);
    }

    fn has_data(&self) -> bool {
        !self.segments.is_empty()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        mln_wire::Wire::write(self, w);
    }

    fn update(
        &self,
        states: &crate::bucket::FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) {
        if let Some(binders) = self.paint_property_binders.get(layer_id) {
            binders.write().update_vertex_vectors(states, layer, image_positions);
            self.base.uploaded.store(false, Ordering::SeqCst);

            // sharedVertices->updateModified()
            self.base.version.fetch_add(1, Ordering::SeqCst);
        }
    }
}
