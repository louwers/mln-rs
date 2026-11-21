use std::sync::{Arc, Once};

use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE,
    ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE,
    ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
};
use mln_renderer::layer::{
    Bucket, BucketBase, CrossFadedPaintPropertyBinder, EvaluatedLayer, GeometryUpload, GeometryVec, Indexed,
    PaintPropertyBinder,
};
use mln_renderer::unstable::{LineLayoutVertex, LineOptions, add_line, subdivide_vertex_line};
use mln_style::expression::{EvaluationContext, Image, Value};
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryTileFeature};
use mln_util::Color;
use mln_util::feature::numeric_value_f64;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecMap;

use super::Line;
use super::dash_array_binder::DashArrayBinder;
use super::properties::{LineLayoutProperties, LinePaintProperties};
use crate::pattern_layout::{PatternBucket, PatternLayerMap};
use crate::tessellator::Tessellator;

mln_renderer::paint_property_binders!(
    pub(crate) struct LineBinders for LinePaintProperties::PossiblyEvaluated {
        line_blur: PaintPropertyBinder<f32> => [ID_LINE_BLUR_VERTEX_ATTRIBUTE],
        line_color: PaintPropertyBinder<Color> => [ID_LINE_COLOR_VERTEX_ATTRIBUTE],
        line_floor_width: PaintPropertyBinder<f32> => [ID_LINE_FLOOR_WIDTH_VERTEX_ATTRIBUTE],
        line_gap_width: PaintPropertyBinder<f32> => [ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE],
        line_offset: PaintPropertyBinder<f32> => [ID_LINE_OFFSET_VERTEX_ATTRIBUTE],
        line_opacity: PaintPropertyBinder<f32> => [ID_LINE_OPACITY_VERTEX_ATTRIBUTE],
        line_pattern: CrossFadedPaintPropertyBinder<Image> => [ID_LINE_PATTERN_FROM_VERTEX_ATTRIBUTE, ID_LINE_PATTERN_TO_VERTEX_ATTRIBUTE],
        line_width: PaintPropertyBinder<f32> => [ID_LINE_WIDTH_VERTEX_ATTRIBUTE],
        line_dasharray: DashArrayBinder => [ID_LINE_DASHARRAY_VERTEX_ATTRIBUTE],
    }
);

#[derive(Clone, mln_wire::Wire)]
pub struct LineBucket {
    pub(crate) base: BucketBase,
    pub(crate) layout: LineLayoutProperties::PossiblyEvaluated,
    pub(crate) vertices: GeometryVec<LineLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: VecMap<String, LineBinders>,
    zoom: f32,
    overscaling: u32,
    subdivision_granularity: SubdivisionGranularitySetting,
}

impl LineBucket {
    #[must_use]
    pub(crate) fn new(
        layout: LineLayoutProperties::PossiblyEvaluated,
        layer_paint_properties: &VecMap<String, Arc<EvaluatedLayer<Line>>>,
        zoom: f32,
        overscaling: u32,
    ) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, properties) in layer_paint_properties {
            paint_property_binders.entry(id.clone()).or_insert_with(|| {
                let mut binders = LineBinders::new(&properties.paint, zoom);
                binders.line_dasharray.set_line_cap(&layout.line_cap);
                binders
            });
        }
        LineBucket {
            base: BucketBase::default(),
            layout,
            vertices: GeometryVec::default(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            paint_property_binders,
            zoom,
            overscaling,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
        }
    }

    /// How far outside a feature a query for `layer` still hits it, from
    /// the layer's current values and the bucket's statistics of them.
    pub(crate) fn query_radius(&self, layer: &EvaluatedLayer<Line>) -> f32 {
        let evaluated = &layer.paint;
        let binders = self.paint_property_binders.get(&layer.style.id);
        let translate = evaluated.line_translate.uniform();
        let offset = evaluated_or_max(
            evaluated.line_offset.constant(),
            LinePaintProperties::Defaults::line_offset(),
            binders.and_then(|b| b.line_offset.max()),
        );
        let mut line_width = evaluated_or_max(
            evaluated.line_width.constant(),
            LinePaintProperties::Defaults::line_width(),
            binders.and_then(|b| b.line_width.max()),
        );
        let gap_width = evaluated_or_max(
            evaluated.line_gap_width.constant(),
            LinePaintProperties::Defaults::line_gap_width(),
            binders.and_then(|b| b.line_gap_width.max()),
        );
        if gap_width != 0.0 {
            line_width = gap_width + 2.0 * line_width;
        }

        line_width / 2.0 + offset.abs() + (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }

    pub(crate) fn set_subdivision_granularity(&mut self, value: &SubdivisionGranularitySetting) {
        self.subdivision_granularity = *value;
    }

    fn add_geometry(
        &mut self,
        line: &[Point<i16>],
        feature: &dyn GeometryTileFeature,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        let closed = feature.get_type() == FeatureType::Polygon;
        if line.len() < if closed { 3 } else { 2 } {
            // A line layer can't draw a source's points, such as a GeoJSON
            // `MultiPoint`. The layer and source aren't known here.
            static WARNED_INVALID_GEOMETRY: Once = Once::new();
            WARNED_INVALID_GEOMETRY.call_once(|| Log::warning(Event::General, "Invalid geometry in line layer"));
            return;
        }
        let granularity = self.subdivision_granularity.line.granularity_for_zoom_level(canonical.z);
        let subdivided;
        let line = if granularity >= 2 {
            subdivided = subdivide_vertex_line(line, granularity, false);
            &subdivided
        } else {
            line
        };

        let clip = match (feature.get_value("mapbox_clip_start"), feature.get_value("mapbox_clip_end")) {
            (Some(start), Some(end)) => numeric_value_f64(&start).zip(numeric_value_f64(&end)),
            _ => None,
        };
        let context =
            EvaluationContext::with_zoom_feature(self.zoom, Some(feature)).with_canonical_tile_id(Some(canonical));
        let options = LineOptions {
            closed,
            join: self.layout.line_join.evaluate(&context),
            cap: self.layout.line_cap.evaluate(&context),
            miter_limit: self.layout.line_miter_limit.evaluate(&context),
            round_limit: self.layout.line_round_limit.evaluate(&context),
            overscaling: self.overscaling,
            clip: clip.filter(|(start, end)| start < end),
        };
        let mut triangles = Indexed { indexes: self.triangles.get_mut(), segments: &mut self.segments };
        add_line(self.vertices.get_mut(), &mut triangles, &mut tessellator.line, line, &options);
    }
}

/// A data-driven property's largest value over the bucket's features, from
/// its binder's `statistics_max`, or else its constant value or default.
fn evaluated_or_max(evaluated_constant: Option<f32>, default_value: f32, statistics_max: Option<f32>) -> f32 {
    match statistics_max {
        Some(max) => max,
        None => evaluated_constant.unwrap_or(default_value),
    }
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

impl PatternBucket for LineBucket {
    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry_collection: &GeometryCollection,
        pattern_positions: &ImagePositions,
        pattern_dependencies: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        for line in geometry_collection.iter() {
            self.add_geometry(line, feature, canonical, tessellator);
        }

        let length = self.vertices.len();
        for (id, binders) in &mut self.paint_property_binders {
            binders.populate_vertex_vectors(
                feature,
                length,
                index,
                pattern_positions,
                pattern_dependencies.get(id),
                canonical,
                &EMPTY_FORMATTED_SECTION,
            );
        }
    }
}

impl Bucket for LineBucket {
    fn base(&self) -> &BucketBase {
        &self.base
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
}
