use mln_gfx::gpu::Caps;
use mln_render::layer::{
    Bucket, CrossFadedPaintPropertyBinder, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload,
};
use mln_style::expression::Image;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryCollection;
use mln_util::Color;

use super::mesh::FillBuffers;
use super::properties::paint;
use crate::tessellator::Tessellator;

mln_render::paint_property_binders!(
    pub(crate) struct FillBinders for paint::PossiblyEvaluated {
        fill_color: PaintPropertyBinder<Color> => ["color"],
        fill_opacity: PaintPropertyBinder<f32> => ["opacity"],
        fill_outline_color: PaintPropertyBinder<Color> => ["outline_color"],
        fill_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
    }
);

/// The width of the lines that outline a fill, in pixels: a device that
/// draws narrower lines and rasterizes on the GPU draws each segment of an
/// outline as a quad that covers that width ([`draws_outline_quads`]).
pub(crate) const LINE_WIDTH: f32 = 2.0;

/// Whether a device of `caps` draws a fill's outlines as quads rather than
/// as lines. One that rasterizes on the CPU, where a quad costs about as
/// much as a draw, draws its narrower lines.
pub(crate) fn draws_outline_quads(caps: &Caps) -> bool {
    caps.max_line_width < LINE_WIDTH && !caps.cpu_rasterizer
}

/// A tile's fills: the triangles that fill its polygons and the lines that
/// outline them, over the same vertices.
#[derive(Clone, Default, mln_wire::Wire)]
pub struct FillBucket {
    pub(crate) vertices: GpuVec<[i16; 2]>,
    /// Where the outline segment that starts at each vertex ends, the
    /// vertex itself where none does: an outline drawn as quads draws one
    /// from each vertex.
    pub(crate) outline_ends: GpuVec<[i16; 2]>,
    pub(crate) triangles: Indexes,
    pub(crate) lines: Indexes,
    pub(crate) binders: LayerBinders<FillBinders>,
    /// Whether the patterns of each layer with a pattern are SDF images,
    /// which draw in the layer's fill color.
    pub(crate) sdf: Vec<(String, bool)>,
}

impl FillBucket {
    /// Adds the meshes of the polygons of `geometry`, in the tile
    /// `canonical`, subdivided on a grid of `granularity` cells per axis
    /// for the globe where it is 2 or more.
    pub(crate) fn add(
        &mut self,
        geometry: &GeometryCollection,
        canonical: &CanonicalTileID,
        granularity: u32,
        tessellator: &mut Tessellator,
    ) {
        FillBuffers {
            vertices: self.vertices.get_mut(),
            outline_ends: self.outline_ends.get_mut(),
            triangles: &mut self.triangles,
            lines: &mut self.lines,
        }
        .add_polygons(geometry, canonical, granularity, tessellator);
    }

    /// Whether the patterns of the layer `layer` are SDF images.
    pub(crate) fn sdf(&self, layer: &str) -> bool {
        self.sdf.iter().any(|(id, sdf)| id == layer && *sdf)
    }
}

impl Bucket for FillBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        match up.caps() {
            Some(caps) if !draws_outline_quads(caps) => self.outline_ends = GpuVec::default(),
            _ => up.vec(&mut self.outline_ends),
        }
        up.indexes(&mut self.triangles);
        up.indexes(&mut self.lines);
        up.binders(&mut self.binders);
    }
}
