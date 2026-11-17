use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_text::glyph::FontFaces;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

/// What a source's update gives its tiles.
pub(crate) struct TileParameters<'a> {
    pub(crate) pixel_ratio: f32,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) transform_state: &'a TransformState,
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
}
