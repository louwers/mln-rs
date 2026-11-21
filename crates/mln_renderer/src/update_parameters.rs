use std::sync::Arc;

use mln_map_types::{Elevation, MapDebugOptions, MapMode, TerrainSkirtLength, TileLodMode, View};
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
    /// draws for speed rather than crispness: from a gesture's start, held
    /// still or not, to its momentum's end, and while a command moves the
    /// center, zoom or bearing. Such a camera reads the terrain away from
    /// its center, which [`elevation`](Self::elevation) says how a frame
    /// waits on. A command of the pitch or the altitude alone, which
    /// doesn't count, runs as an animation, which keeps a continuous map
    /// from going idle, and a still image shows it ended.
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
    /// Identifies the update among its map's, which number them from 1 in
    /// the order they're made. A frame's report names the update it drew
    /// ([`RendererEvent::RenderFrameFinished`]).
    ///
    /// [`RendererEvent::RenderFrameFinished`]: crate::RendererEvent::RenderFrameFinished
    pub update_id: u64,
    pub cross_source_collisions: bool,
    pub fast_pfor_enabled: bool,
    /// How far the skirts of terrain tiles reach down.
    pub terrain_skirt_length: TerrainSkirtLength,
    /// The terrain's ground that `view` stands on: the snapshot of it that
    /// the renderer last reported ([`RendererEvent::TerrainElevation`]) and
    /// the map applied, or `None`. A frame is complete only when this gives
    /// the same height and floor at the view's center as the renderer's
    /// current snapshot, or both are `None`, and, while the camera moves
    /// ([`camera_moving`](Self::camera_moving)), only when this is that
    /// snapshot. A still image draws no other.
    ///
    /// [`RendererEvent::TerrainElevation`]: crate::RendererEvent::TerrainElevation
    pub elevation: Option<Arc<dyn Elevation>>,

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
