use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode, View};
use mln_style::Style;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::time::Instant;

#[derive(Clone)]
pub struct UpdateParameters {
    pub style_loaded: bool,
    pub mode: MapMode,
    pub pixel_ratio: f32,
    pub debug_options: MapDebugOptions,
    /// The frame's time; `None` in still modes, which have no clock and show
    /// every transition and fade as ended.
    pub time_point: Option<Instant>,
    /// The view the frame shows.
    pub view: View,
    /// The map's zoom range, which the sources load tiles within.
    pub min_zoom: f64,
    pub max_zoom: f64,
    /// Whether a gesture or an animation moves the camera, which the frame
    /// draws for speed rather than crispness.
    pub camera_moving: bool,

    /// The version of the style to draw. A renderer compares it with the
    /// version it drew last ([`mln_style::diff::StyleDiff`]).
    pub style: Arc<Style>,
    pub sprite_loaded: bool,
    /// The map's feature state. Each render source takes its own, in place
    /// of what it held.
    pub feature_states: Arc<crate::source_state::FeatureStatesBySource>,

    pub file_source: Option<Arc<dyn mln_storage::FileSource>>,
    pub prefetch_zoom_delta: u8,

    /// For still image requests, render requested.
    pub still_image_request: bool,
    /// Identifies the still image request: each new one retries the
    /// resources that failed to load.
    pub still_image_request_id: u64,
    pub cross_source_collisions: bool,
    pub fast_pfor_enabled: bool,
    /// How far the skirts of terrain tiles reach down.
    pub terrain_skirt_length: TerrainSkirtLength,
    /// Counts the map's moves of its camera onto the terrain: the frame of
    /// the update reports it back ([`RendererEvent::RenderFrameFinished`]).
    ///
    /// [`RendererEvent::RenderFrameFinished`]: crate::RendererEvent::RenderFrameFinished
    pub ground_generation: u64,

    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_zoom_shift: f64,
    pub tile_lod_mode: TileLodMode,

    pub subdivision_granularity: SubdivisionGranularitySetting,
}

// The map and a renderer on another thread share an update.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<UpdateParameters>();
};
