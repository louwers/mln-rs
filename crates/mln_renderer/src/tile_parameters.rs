use std::sync::Arc;

use mln_map_types::{MapMode, TileLodMode, View};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_text::glyph::FontFaces;

use crate::terrain::TerrainCover;

/// What a source's update gives its tiles.
pub(crate) struct TileParameters<'a> {
    pub(crate) pixel_ratio: f32,
    pub(crate) view: &'a View,
    /// The map's zoom range, which tiles load within.
    pub(crate) min_zoom: f64,
    pub(crate) max_zoom: f64,
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    pub(crate) mode: MapMode,
    pub(crate) prefetch_zoom_delta: u8,
    pub(crate) tile_lod_min_radius: f64,
    pub(crate) tile_lod_scale: f64,
    pub(crate) tile_lod_pitch_threshold: f64,
    pub(crate) tile_lod_zoom_shift: f64,
    pub(crate) tile_lod_mode: TileLodMode,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
    pub(crate) global_state: Option<Arc<GlobalStateMap>>,
    pub(crate) is_update_synchronous: bool,
    /// The style's font faces, which the tiles' layouts shape text with.
    pub(crate) font_faces: Option<Arc<FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    /// How many placements have committed before the update
    /// (`PlacementController::commits`), which the tiles the view leaves
    /// stay rendered for while their symbols fade out.
    pub(crate) placement_commits: u64,
    /// The terrain's cover while terrain draws, whose elevation ranges cull
    /// the sources' covers.
    pub(crate) terrain: Option<&'a TerrainCover>,
}
