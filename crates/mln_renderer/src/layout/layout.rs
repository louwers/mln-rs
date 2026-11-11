// Ports src/mln/layout/layout.hpp.
//
// C++ hands `createLayout` a `std::unique_ptr<GeometryTileLayer>` that owns
// (a share of) the tile data. A Rust `GeometryTileLayer` borrows its
// `GeometryTileData`, and the worker keeps layouts with pending
// dependencies across messages, so layouts get a `SourceLayer`: the shared
// tile data and the layer name, from which they re-borrow the layer (and
// re-get features by index) when they need it. Same results.

use std::collections::HashMap;
use std::sync::Arc;

use mln_style_types::image::{ImageDependencies, ImageMap, ImagePositions};
use mln_text::glyph::{GlyphDependencies, GlyphMap, GlyphPositions};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryTileData, GeometryTileLayer};
use mln_util::exception::{ExceptionPtr, RuntimeError};
use mln_vec_map::VecSet;

use crate::bucket_parameters::BucketParameters;
use crate::geometry::feature_index::FeatureIndex;
use crate::layout::filter_groups::FilterGroups;
use crate::render_layer::LayerRenderData;
use crate::text::glyph_manager::HBShapeResults;

/// The `std::unique_ptr<GeometryTileLayer>` a layout keeps.
#[derive(Clone)]
pub struct SourceLayer {
    pub data: Arc<dyn GeometryTileData + Send + Sync>,
    pub name: String,
}

impl SourceLayer {
    pub fn new(data: Arc<dyn GeometryTileData + Send + Sync>, name: impl Into<String>) -> Self {
        SourceLayer { data, name: name.into() }
    }

    /// The layer (`data->getLayer(name)`). The worker only creates layouts
    /// for layers the data has, so neither error is expected.
    pub fn layer(&self) -> Result<Box<dyn GeometryTileLayer + '_>, ExceptionPtr> {
        self.data
            .get_layer(&self.name)?
            .ok_or_else(|| RuntimeError::ptr(format!("SourceLayer: the tile data has no layer {:?}", self.name)))
    }
}

/// `Layout`.
pub trait Layout: Send {
    /// `createBucket(patternPositions, featureIndex, renderData, firstLoad,
    /// showCollisionBoxes, canonical)`. `Err` is an exception C++ lets
    /// propagate (e.g. `GeometryTooLongException` from `addFeature`); the
    /// worker reports it as the tile's error.
    fn create_bucket(
        &mut self,
        pattern_positions: &ImagePositions,
        feature_index: &mut FeatureIndex,
        render_data: &mut HashMap<String, LayerRenderData>,
        first_load: bool,
        show_collision_boxes: bool,
        canonical: &CanonicalTileID,
    ) -> Result<(), ExceptionPtr>;

    fn has_symbol_instances(&self) -> bool {
        true
    }

    fn has_dependencies(&self) -> bool;

    /// `prepareSymbols(glyphMap, glyphPositions, imageMap, imagePositions)`
    /// (symbol layouts; the default does nothing, as in C++).
    fn prepare_symbols(
        &mut self,
        _glyph_map: &GlyphMap,
        _glyph_positions: &GlyphPositions,
        _image_map: &ImageMap,
        _image_positions: &ImagePositions,
    ) {
    }

    /// `needFinalizeSymbols()`.
    fn need_finalize_symbols(&self) -> bool {
        false
    }

    /// `finalizeSymbols(HBShapeResults&)`.
    fn finalize_symbols(&mut self, _results: &mut HBShapeResults) {}
}

/// `LayoutParameters`.
pub struct LayoutParameters<'a> {
    pub bucket_parameters: &'a BucketParameters,
    /// symbols: `std::shared_ptr<FontFaces> fontFaces`.
    pub font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    pub glyph_dependencies: &'a mut GlyphDependencies,
    pub image_dependencies: &'a mut ImageDependencies,
    pub available_images: &'a VecSet<String>,
    /// Port-only: the tile's feature groups, shared by the layouts' filters.
    pub filter_groups: &'a mut FilterGroups,
}
