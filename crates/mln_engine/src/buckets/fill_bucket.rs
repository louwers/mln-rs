use mln_style::color::Color;
use mln_style::expression::Image;
use mln_style::feature::{GeometryCollection, GeometryCoordinate};
use mln_style::hash::{HashMap, HashSet};
use mln_style::layers::fill_layer_properties::FillPaintProperties;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_wire::sorted;

use crate::draw::mesh::Indexed;
use crate::draw::segment::SegmentBase;
use crate::gfx::fill_mesh::FillBuffers;
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::GeometryVec;
use crate::layer::paint_property_binder::{CrossFadedPaintPropertyBinder, LayerBinders, PaintPropertyBinder};
use crate::layers::render_fill_layer::draws_outline_quads;
use crate::log_target;
use crate::util::tessellator::Tessellator;

crate::paint_property_binders!(
    pub struct FillBinders for FillPaintProperties::PossiblyEvaluated {
        fill_color: PaintPropertyBinder<Color> => ["color"],
        fill_opacity: PaintPropertyBinder<f32> => ["opacity"],
        fill_outline_color: PaintPropertyBinder<Color> => ["outline_color"],
        fill_pattern: CrossFadedPaintPropertyBinder<Image> => ["pattern_from", "pattern_to"],
    }
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FillLayoutVertex {
    pub(crate) a1: [i16; 2],
}

impl FillLayoutVertex {
    pub(crate) fn new(p: GeometryCoordinate) -> Self {
        FillLayoutVertex { a1: [p.x, p.y] }
    }
}

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct FillBucket {
    pub(crate) basic_lines: GeometryVec<u16>,
    pub(crate) basic_line_segments: Vec<SegmentBase>,

    pub(crate) vertices: GeometryVec<FillLayoutVertex>,
    /// Where the outline segment that starts at each vertex ends, the
    /// vertex itself where none does: an instanced outline draws a segment
    /// per vertex. A device that draws the outlines as lines discards them.
    pub(crate) outline_ends: GeometryVec<FillLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) triangle_segments: Vec<SegmentBase>,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,

    pub(crate) paint_property_binders: LayerBinders<FillBinders>,

    #[wire(with = "sorted::map")]
    sdf_patterns: HashMap<String, bool>,
    #[wire(with = "sorted::set")]
    mixed_sdf_pattern_layers: HashSet<String>,
}

impl FillBucket {
    /// A bucket with no features of the layers of `paint_property_binders`,
    /// which splits its polygons as `subdivision_granularity` says.
    #[must_use]
    pub(crate) fn new(
        paint_property_binders: LayerBinders<FillBinders>,
        subdivision_granularity: SubdivisionGranularitySetting,
    ) -> Self {
        FillBucket {
            basic_lines: GeometryVec::default(),
            basic_line_segments: Vec::new(),
            vertices: GeometryVec::default(),
            outline_ends: GeometryVec::default(),
            triangles: GeometryVec::default(),
            triangle_segments: Vec::new(),
            subdivision_granularity,
            paint_property_binders,
            sdf_patterns: HashMap::default(),
            mixed_sdf_pattern_layers: HashSet::default(),
        }
    }

    /// Adds the polygons of `geometry`, a feature's in the tile `canonical`.
    pub(crate) fn add_geometry(
        &mut self,
        geometry: &GeometryCollection,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        let mut buffers = FillBuffers {
            vertices: self.vertices.get_mut(),
            outline_ends: self.outline_ends.get_mut(),
            triangles: Indexed { indexes: self.triangles.get_mut(), segments: &mut self.triangle_segments },
            lines: Indexed { indexes: self.basic_lines.get_mut(), segments: &mut self.basic_line_segments },
        };
        let granularity = self.subdivision_granularity.fill.granularity_for_zoom_level(canonical.z);
        buffers.add_polygons(geometry, canonical, granularity, tessellator);
    }

    /// Whether the bucket has anything to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        !self.triangle_segments.is_empty() || !self.basic_line_segments.is_empty()
    }

    /// Records whether the layer `layer_id` draws a pattern that is an SDF
    /// image; a layer can't mix both.
    pub(crate) fn record_sdf_pattern(&mut self, layer_id: &str, sdf: bool) {
        match self.sdf_patterns.get(layer_id) {
            None => {
                self.sdf_patterns.insert(layer_id.to_owned(), sdf);
            }
            Some(&existing) => {
                if existing != sdf && self.mixed_sdf_pattern_layers.insert(layer_id.to_owned()) {
                    log::warn!(
                        target: log_target::STYLE,
                        "Style sheet warning: Cannot mix SDF and non-SDF fill patterns in layer \"{layer_id}\""
                    );
                }
            }
        }
    }

    /// Whether the layer `layer_id`'s patterns are SDF images.
    #[must_use]
    pub(crate) fn is_sdf_pattern(&self, layer_id: &str) -> bool {
        self.sdf_patterns.get(layer_id).copied().unwrap_or(false)
    }
}

impl Bucket for FillBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.vertices);
        up.indexes(&mut self.triangles);
        up.indexes(&mut self.basic_lines);
        if up.caps().is_some_and(draws_outline_quads) {
            up.vec(&mut self.outline_ends);
        } else if up.caps().is_some() {
            // Only the instanced outlines read the outline ends.
            self.outline_ends.discard();
        }
        up.binders(&mut self.paint_property_binders);
    }
}
