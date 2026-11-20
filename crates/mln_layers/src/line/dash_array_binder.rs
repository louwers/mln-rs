//! The binder of `line-dasharray`.

use mln_gfx::gfx_types::AttributeDataType;
use mln_renderer::draw::{SharedAttribute, SharedVertices};
use mln_renderer::layer::{InterleavedVertexBuffer, PaintPropertyBinderBase, PatternDependency};
use mln_renderer::unstable::{DashAtlas, LinePatternCap};
use mln_style::evaluated::Evaluated;
use mln_style::expression::{EvaluationContext, Value};
use mln_style::property_expression::PropertyExpression;
use mln_style_types::LineCapType;
use mln_style_types::image::ImagePositions;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;

/// The binder of `line-dasharray`. Where the dash array differs by feature,
/// each feature's vertices hold its pattern's entry in the bucket's
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
    /// Where the dashes are in each vertex of the bucket's interleaved
    /// buffer, once the binder has a place there.
    vertex_offset: Option<usize>,
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
                vertex_offset: None,
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
            DashArrayBinder::PerFeature(dashes) => dashes.vertex_offset,
        }
    }
}

impl PaintPropertyBinderBase for DashArrayBinder {
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer) {
        let DashArrayBinder::PerFeature(dashes) = self else {
            return;
        };
        let new_vertex_offset = buffer.extend_vertex_format(DASH_VERTEX_SIZE, align_of::<f32>());
        dashes.vertex_offset.get_or_insert(new_vertex_offset);
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
        let PerFeatureDashes { expression, default_value, zoom, cap, atlas, vertex_offset } = &mut **dashes;
        let Some(vertex_offset) = *vertex_offset else {
            return;
        };
        let context =
            EvaluationContext::with_zoom_feature(*zoom, Some(feature)).with_canonical_tile_id(Some(canonical));
        let dasharray = expression.evaluate(&context, default_value.clone());
        let cap =
            if cap.evaluate(&context) == LineCapType::Round { LinePatternCap::Round } else { LinePatternCap::Square };
        let entry = atlas.add(&dasharray, cap);
        let value = [entry.y, entry.height, entry.width, 0.0];
        buffer.fill(buffer.vertex_count()..length, vertex_offset, bytemuck::cast_slice(&value));
    }

    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        if self.vertex_offset().is_some() { buffer.vertex_count() } else { 0 }
    }

    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedVertices) -> Option<SharedAttribute> {
        let vertex_offset = self.vertex_offset()?;
        Some(SharedAttribute::new(
            shared.clone(),
            AttributeDataType::Float4,
            vertex_offset as u32,
            buffer.stride() as u32,
        ))
    }
}
