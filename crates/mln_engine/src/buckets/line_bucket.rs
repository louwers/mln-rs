use std::sync::{Arc, Once};

use geometry::Point;
use mln_style::color::Color;
use mln_style::expression::{EvaluationContext, Image, Value};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::geometry::FeatureType;
use mln_style::layers::Line;
use mln_style::layers::line_layer_properties::{LineLayoutProperties, LinePaintProperties};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_util::feature::numeric_value_f64;
use mln_vec_map::VecMap;

use crate::draw::mesh::Indexed;
use crate::draw::segment::SegmentBase;
use crate::gfx::line_mesh::{LineLayoutVertex, LineOptions, add_line};
use crate::layer::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::layer::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::{CrossFadedPaintPropertyBinder, DashArrayBinder, PaintPropertyBinder};
use crate::layout::pattern_layout::PatternLayerMap;
use crate::log_target;
use crate::render::globe::subdivision::subdivide_vertex_line;
use crate::text::atlas::ImagePositions;
use crate::util::tessellator::Tessellator;

crate::paint_property_binders!(
    pub struct LineBinders for LinePaintProperties::PossiblyEvaluated {
        line_blur: PaintPropertyBinder<f32> => ["blur"],
        line_color: PaintPropertyBinder<Color> => ["color"],
        line_floor_width: PaintPropertyBinder<f32> => ["floorwidth"],
        line_gap_width: PaintPropertyBinder<f32> => ["gapwidth"],
        line_offset: PaintPropertyBinder<f32> => ["offset"],
        line_opacity: PaintPropertyBinder<f32> => ["opacity"],
        line_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
        line_width: PaintPropertyBinder<f32> => ["width"],
        line_dasharray: DashArrayBinder => ["dasharray"],
    }
);

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct LineBucket {
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
            binders.and_then(|b| b.line_offset.statistics.max()),
        );
        let mut line_width = evaluated_or_max(
            evaluated.line_width.constant(),
            LinePaintProperties::Defaults::line_width(),
            binders.and_then(|b| b.line_width.statistics.max()),
        );
        let gap_width = evaluated_or_max(
            evaluated.line_gap_width.constant(),
            LinePaintProperties::Defaults::line_gap_width(),
            binders.and_then(|b| b.line_gap_width.statistics.max()),
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
            WARNED_INVALID_GEOMETRY
                .call_once(|| log::warn!(target: log_target::GENERAL, "Invalid geometry in line layer"));
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

impl Bucket for LineBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

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
