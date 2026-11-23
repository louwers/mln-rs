use std::sync::Arc;

use mln_map_types::{MapDebugOptions, TileLodMode, View};
use mln_style::expression::GlobalStateMap;
use mln_text::glyph::FontFaces;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::source::MapSettings;

/// What a source's update gives its tiles.
pub(crate) struct TileParameters<'a> {
    pub(crate) map: MapSettings,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) view: &'a View,
    /// The map's lowest zoom level; its highest is the map settings'.
    pub(crate) min_zoom: f64,
    pub(crate) prefetch_zoom_delta: u8,
    pub(crate) tile_lod_min_radius: f64,
    pub(crate) tile_lod_scale: f64,
    pub(crate) tile_lod_pitch_threshold: f64,
    pub(crate) tile_lod_zoom_shift: f64,
    pub(crate) tile_lod_mode: TileLodMode,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
    /// Whether the renderer's device rasterizes on the CPU
    /// (`Caps::cpu_rasterizer`), which the tiles' layouts read.
    pub(crate) cpu_rasterizer: bool,
    pub(crate) global_state: Option<Arc<GlobalStateMap>>,
    /// The style's font faces, which the tiles' layouts shape text with.
    pub(crate) font_faces: Option<Arc<FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    /// How many placements have committed before the update
    /// (`Schedule::commits`), which the tiles the view leaves stay
    /// rendered for while what placement showed of them fades out.
    pub(crate) placement_commits: u64,
}
