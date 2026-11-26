use std::sync::Once;

use geometry::Point;
use mln_gfx::gfx_types::AttributeDataType;
use mln_style::color::Color;
use mln_style::expression::{EvaluationContext, Image, Value};
use mln_style::feature::GeometryTileFeature;
use mln_style::geometry::FeatureType;
use mln_style::layers::Line;
use mln_style::layers::line_layer_properties::{LineLayoutProperties, LinePaintProperties};
use mln_style::property::evaluated::Evaluated;
use mln_style::property::expression::PropertyExpression;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_style::types::LineCapType;

use crate::draw::mesh::Indexed;
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::SharedAttribute;
use crate::draw::vertices::SharedVertices;
use crate::gfx::line_mesh::{LineLayoutVertex, LineOptions, add_line};
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::GeometryVec;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::paint_property_binder::{
    CrossFadedPaintPropertyBinder, InterleavedVertexBuffer, InterleavedVertexData, LayerBinders, PaintPropertyBinder,
    PaintPropertyBinderBase, shared_paint,
};
use crate::layers::line::dashes::{DashAtlas, LinePatternCap};
use crate::layout::pattern_layout::PatternDependency;
use crate::log_target;
use crate::render::globe::subdivision::subdivide_vertex_line;
use crate::text::atlas::ImagePositions;
use crate::tile::OverscaledTileID;
use crate::tile::feature::numeric_value_f64;
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

/// The binder of `line-dasharray`. Where the dash array differs by feature,
/// each feature's vertices hold its pattern's [`DashEntry`] in the bucket's
/// [`DashAtlas`], as four floats with the last unused.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) enum DashArrayBinder {
    Constant,
    PerFeature(Box<PerFeatureDashes>),
}

/// The data of a [`DashArrayBinder`] whose dash array differs by feature.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct PerFeatureDashes {
    expression: PropertyExpression<Vec<f32>>,
    default_value: Vec<f32>,
    zoom: f32,
    cap: Evaluated<LineCapType>,
    atlas: DashAtlas,
    vertex_data: InterleavedVertexData,
}

/// The size of a dash vertex: four floats.
const DASH_VERTEX_SIZE: usize = 16;

impl DashArrayBinder {
    #[must_use]
    pub(crate) fn create(value: &Evaluated<Vec<f32>>, zoom: f32) -> Self {
        match value {
            Evaluated::Constant(_) | Evaluated::CrossFaded(_) => DashArrayBinder::Constant,
            Evaluated::PerFeature { expression, default } => DashArrayBinder::PerFeature(Box::new(PerFeatureDashes {
                expression: expression.clone(),
                default_value: default.clone(),
                zoom,
                cap: Evaluated::Constant(LineCapType::Butt),
                atlas: DashAtlas::default(),
                vertex_data: InterleavedVertexData::Uninitialized,
            })),
        }
    }

    /// Sets the `line-cap` that gives a feature round dashes.
    pub(crate) fn set_line_cap(&mut self, line_cap: &Evaluated<LineCapType>) {
        if let DashArrayBinder::PerFeature(dashes) = self {
            dashes.cap.clone_from(line_cap);
        }
    }

    /// The patterns of the features, if the dash array differs by feature.
    #[must_use]
    pub(crate) fn atlas(&self) -> Option<&DashAtlas> {
        match self {
            DashArrayBinder::Constant => None,
            DashArrayBinder::PerFeature(dashes) => Some(&dashes.atlas),
        }
    }

    fn vertex_offset(&self) -> Option<usize> {
        match self {
            DashArrayBinder::Constant => None,
            DashArrayBinder::PerFeature(dashes) => dashes.vertex_data.offset(),
        }
    }
}

impl PaintPropertyBinderBase for DashArrayBinder {
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer) {
        let DashArrayBinder::PerFeature(dashes) = self else {
            return;
        };
        let vertex_data = &mut dashes.vertex_data;
        let new_vertex_offset = buffer.extend_vertex_format(DASH_VERTEX_SIZE, align_of::<f32>());
        if matches!(vertex_data, InterleavedVertexData::Uninitialized) {
            *vertex_data = InterleavedVertexData::Interleaved { offset: new_vertex_offset };
        }
    }

    fn populate(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        feature: &dyn GeometryTileFeature,
        length: usize,
        _index: usize,
        _pattern_positions: &ImagePositions,
        _pattern_dependencies: Option<&PatternDependency>,
        canonical: &CanonicalTileID,
        _formatted_section: &Value,
    ) {
        let DashArrayBinder::PerFeature(dashes) = self else {
            return;
        };
        let PerFeatureDashes { expression, default_value, zoom, cap, atlas, vertex_data } = &mut **dashes;
        let Some(vertex_offset) = vertex_data.offset() else {
            return;
        };
        let context =
            EvaluationContext::with_zoom_feature(*zoom, Some(feature)).with_canonical_tile_id(Some(canonical));
        let dasharray = expression.evaluate(&context, default_value.clone());
        let cap =
            if cap.evaluate(&context) == LineCapType::Round { LinePatternCap::Round } else { LinePatternCap::Square };
        let entry = atlas.add(&dasharray, cap);
        let value = [entry.y, entry.height, entry.width, 0.0];
        buffer.fill(buffer.vertex_count..length, vertex_offset, bytemuck::cast_slice(&value));
    }

    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        if self.vertex_offset().is_some() { buffer.vertex_count } else { 0 }
    }

    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedVertices) -> Option<SharedAttribute> {
        let vertex_offset = self.vertex_offset()?;
        Some(shared_paint(shared, vertex_offset, buffer.stride as u32, AttributeDataType::Float4))
    }
}

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct LineBucket {
    pub(crate) layout: LineLayoutProperties::PossiblyEvaluated,
    pub(crate) vertices: GeometryVec<LineLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) segments: Vec<SegmentBase>,
    pub(crate) paint_property_binders: LayerBinders<LineBinders>,
    zoom: f32,
    overscaling: u32,
    subdivision_granularity: SubdivisionGranularitySetting,
}

impl LineBucket {
    /// A bucket with no features, for the tile `tile`, of the layers of
    /// `paint_property_binders`, laid out as `layout` says and split as
    /// `subdivision_granularity` says.
    #[must_use]
    pub(crate) fn new(
        layout: LineLayoutProperties::PossiblyEvaluated,
        mut paint_property_binders: LayerBinders<LineBinders>,
        tile: OverscaledTileID,
        subdivision_granularity: SubdivisionGranularitySetting,
    ) -> Self {
        for binders in paint_property_binders.values_mut() {
            binders.line_dasharray.set_line_cap(&layout.line_cap);
        }
        LineBucket {
            layout,
            vertices: GeometryVec::default(),
            triangles: GeometryVec::default(),
            segments: Vec::new(),
            paint_property_binders,
            zoom: f32::from(tile.overscaled_z),
            overscaling: tile.overscale_factor(),
            subdivision_granularity,
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

    /// Whether the bucket has anything to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Adds `line`, a line of `feature`'s geometry in the tile `canonical`.
    pub(crate) fn add_geometry(
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

impl Bucket for LineBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.paint_property_binders);
    }
}
