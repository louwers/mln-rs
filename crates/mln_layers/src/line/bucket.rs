use mln_gfx::gfx_types::AttributeDataType;
use mln_render::draw::Vertex;
use mln_render::layer::{
    Bucket, CrossFadedPaintPropertyBinder, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload,
};
use mln_style::expression::Image;
use mln_util::Color;

use super::LinePaintProperties;
use super::dashes::{DashAtlas, DashCap};
use super::mesh::LineVertex;

mln_render::paint_property_binders!(
    pub(crate) struct LineBinders for LinePaintProperties::PossiblyEvaluated {
        line_blur: PaintPropertyBinder<f32> => ["blur"],
        line_color: PaintPropertyBinder<Color> => ["color"],
        line_floor_width: PaintPropertyBinder<f32> => ["floorwidth"],
        line_gap_width: PaintPropertyBinder<f32> => ["gapwidth"],
        line_offset: PaintPropertyBinder<f32> => ["offset"],
        line_opacity: PaintPropertyBinder<f32> => ["opacity"],
        line_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
        line_width: PaintPropertyBinder<f32> => ["width"],
    }
);

/// A tile's lines: a strip of triangles along each, whose vertices hold
/// the direction to extrude them in.
#[derive(Clone, Default, mln_wire::Wire)]
pub struct LineBucket {
    pub(crate) vertices: GpuVec<LineVertex>,
    pub(crate) triangles: Indexes,
    pub(crate) binders: LayerBinders<LineBinders>,
    /// The dash patterns of each layer whose `line-dasharray` differs by
    /// feature.
    pub(crate) dashes: Vec<FeatureDashes>,
    /// The ends of the dash patterns that the bucket shares: round where
    /// its `line-cap` is round for every feature.
    pub(crate) dash_cap: DashCap,
}

/// The dash patterns of a layer's features, for a `line-dasharray` that
/// differs by feature: an atlas of them, and where each vertex's is.
#[derive(Clone, Default, mln_wire::Wire)]
pub(crate) struct FeatureDashes {
    /// The layer's id.
    pub(crate) layer: String,
    pub(crate) atlas: DashAtlas,
    pub(crate) vertices: GpuVec<DashVertex>,
}

/// Where a vertex's dash pattern is in its [`DashAtlas`]: its middle row,
/// its rows' span and its length, and a fourth float unused.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct DashVertex(pub(crate) [f32; 4]);

impl Vertex for DashVertex {
    const ATTRIBUTES: &'static [AttributeDataType] = &[AttributeDataType::Float4];
    const NAMES: &'static [&'static str] = &["dasharray"];
}

impl LineBucket {
    /// The dash patterns of the layer `layer`'s features, where its dash
    /// array differs by feature.
    pub(crate) fn dashes(&self, layer: &str) -> Option<&FeatureDashes> {
        self.dashes.iter().find(|dashes| dashes.layer == layer)
    }
}

impl Bucket for LineBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.binders(&mut self.binders);
        for dashes in &mut self.dashes {
            up.vec(&mut dashes.vertices);
        }
    }
}
