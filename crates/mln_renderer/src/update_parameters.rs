use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_style::collection::ImmutableVector;
use mln_style::expression::GlobalStateMap;
use mln_style::transition_options::TransitionOptions;
use mln_style::{image, layer, light, sky, source};
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
    pub transform_state: TransformState,

    pub glyph_url: String,
    /// symbols: `std::shared_ptr<FontFaces> fontFaces`.
    pub font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    pub sprite_loaded: bool,
    pub transition_options: TransitionOptions,
    pub light: Arc<light::Impl>,
    pub sky: Option<Arc<sky::Impl>>,
    pub images: Arc<Vec<Arc<image::Impl>>>,
    pub sources: ImmutableVector<dyn source::Impl>,
    pub layers: ImmutableVector<dyn layer::Impl>,
    /// The style's global state, used by "global-state" expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,
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
    pub capture_rendered_features: bool,

    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_zoom_shift: f64,
    pub tile_lod_mode: TileLodMode,

    pub subdivision_granularity: SubdivisionGranularitySetting,
}

impl std::fmt::Debug for UpdateParameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateParameters")
            .field("style_loaded", &self.style_loaded)
            .field("mode", &self.mode)
            .field("pixel_ratio", &self.pixel_ratio)
            .field("debug_options", &self.debug_options)
            .field("time_point", &self.time_point)
            .field("transform_state", &self.transform_state)
            .field("glyph_url", &self.glyph_url)
            .field("font_faces", &self.font_faces)
            .field("sprite_loaded", &self.sprite_loaded)
            .field("transition_options", &self.transition_options)
            .field("light", &self.light)
            .field("sky", &self.sky)
            .field("images", &self.images)
            .field("sources", &self.sources)
            .field("layers", &self.layers)
            .field("global_state", &self.global_state)
            .field("feature_states", &self.feature_states)
            .field("prefetch_zoom_delta", &self.prefetch_zoom_delta)
            .field("still_image_request", &self.still_image_request)
            .field("still_image_request_id", &self.still_image_request_id)
            .field("cross_source_collisions", &self.cross_source_collisions)
            .field("fast_pfor_enabled", &self.fast_pfor_enabled)
            .field("capture_rendered_features", &self.capture_rendered_features)
            .field("tile_lod_min_radius", &self.tile_lod_min_radius)
            .field("tile_lod_scale", &self.tile_lod_scale)
            .field("tile_lod_pitch_threshold", &self.tile_lod_pitch_threshold)
            .field("tile_lod_zoom_shift", &self.tile_lod_zoom_shift)
            .field("tile_lod_mode", &self.tile_lod_mode)
            .field("subdivision_granularity", &self.subdivision_granularity)
            .finish_non_exhaustive()
    }
}

// The map and a renderer on another thread share an update.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<UpdateParameters>();
};
