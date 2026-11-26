use std::sync::Arc;

use mln_style::color::Color;
use mln_style::expression::{Image, Value};
use mln_style::feature::{GeometryCollection, GeometryCoordinate, GeometryTileFeature};
use mln_style::hash::{HashMap, HashSet};
use mln_style::layers::Fill;
use mln_style::layers::fill_layer_properties::{FillLayoutProperties, FillPaintProperties};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::tile_id::CanonicalTileID;
use mln_text::atlas::ImagePositions;
use mln_vec_map::VecMap;
use mln_wire::sorted;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::gfx::fill_mesh::FillBuffers;
use crate::gfx::mesh::Indexed;
use crate::gfx::segment::SegmentBase;
use crate::layer_type::EvaluatedLayer;
use crate::layout::pattern_layout::PatternLayerMap;
use crate::log_target;
use crate::paint_property_binder::{CrossFadedPaintPropertyBinder, PaintPropertyBinder};
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
    pub(crate) base: BucketBase,

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

    pub(crate) paint_property_binders: VecMap<String, FillBinders>,

    #[wire(with = "sorted::map")]
    sdf_patterns: HashMap<String, bool>,
    #[wire(with = "sorted::set")]
    mixed_sdf_pattern_layers: HashSet<String>,
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

impl FillBucket {
    #[must_use]
    pub(crate) fn new(
        _layout: &FillLayoutProperties::PossiblyEvaluated,
        layer_paint_properties: &VecMap<String, Arc<EvaluatedLayer<Fill>>>,
        zoom: f32,
        _overscaling: u32,
    ) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, properties) in layer_paint_properties {
            paint_property_binders.entry(id.clone()).or_insert_with(|| FillBinders::new(&properties.paint, zoom));
        }
        FillBucket {
            base: BucketBase::default(),
            basic_lines: GeometryVec::default(),
            basic_line_segments: Vec::new(),
            vertices: GeometryVec::default(),
            outline_ends: GeometryVec::default(),
            triangles: GeometryVec::default(),
            triangle_segments: Vec::new(),
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            paint_property_binders,
            sdf_patterns: HashMap::default(),
            mixed_sdf_pattern_layers: HashSet::default(),
        }
    }

    pub(crate) fn set_subdivision_granularity(&mut self, value: &SubdivisionGranularitySetting) {
        self.subdivision_granularity = *value;
    }

    fn generate_buffers(
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

    fn populate_binders(
        &mut self,
        feature: &dyn GeometryTileFeature,
        pattern_positions: &ImagePositions,
        pattern_dependencies: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
    ) {
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

impl Bucket for FillBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        pattern_positions: &ImagePositions,
        pattern_dependencies: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
        tessellator: &mut Tessellator,
    ) {
        self.generate_buffers(geometry, canonical, tessellator);
        self.populate_binders(feature, pattern_positions, pattern_dependencies, index, canonical);
    }

    fn record_sdf_pattern(&mut self, layer_id: &str, sdf: bool) {
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

    fn is_sdf_pattern(&self, layer_id: &str) -> bool {
        self.sdf_patterns.get(layer_id).copied().unwrap_or(false)
    }

    fn upload(&mut self, geometry: &mut GeometryUpload<'_>, needs: UploadNeeds) {
        geometry.move_vertices(&mut self.vertices);
        geometry.move_indexes(&mut self.triangles);
        geometry.move_indexes(&mut self.basic_lines);
        if needs.fill_outline_ends {
            geometry.move_vertices(&mut self.outline_ends);
        } else {
            // Only the instanced outlines read the outline ends.
            self.outline_ends.discard();
        }
        for binders in self.paint_property_binders.values_mut() {
            let depends_on_feature_state = binders.depends_on_feature_state();
            geometry.move_paint(&mut binders.interleaved_vertex_buffer, depends_on_feature_state);
        }
        self.base.uploaded = true;
    }

    fn has_data(&self) -> bool {
        !self.triangle_segments.is_empty() || !self.basic_line_segments.is_empty()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        mln_wire::Wire::write(self, w);
    }

    fn update(
        &mut self,
        states: &crate::bucket::FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
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
