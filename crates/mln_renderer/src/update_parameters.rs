// Ports src/mln/renderer/update_parameters.hpp

use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_style::collection::ImmutableVector;
use mln_style::expression::GlobalStateMap;
use mln_style::transition_options::TransitionOptions;
use mln_style::{image, layer, light, source};
use mln_util::chrono::TimePoint;
use mln_util::immutable::Immutable;

// Port-only Clone: a renderer on another thread than the map (a host's
// graphics thread) receives a copy, since TransformState's caches aren't Sync.
#[derive(Clone)]
pub struct UpdateParameters {
    pub style_loaded: bool,
    pub mode: MapMode,
    pub pixel_ratio: f32,
    pub debug_options: MapDebugOptions,
    pub time_point: TimePoint,
    pub transform_state: TransformState,

    pub glyph_url: String,
    /// symbols: `std::shared_ptr<FontFaces> fontFaces`.
    pub font_faces: Option<std::sync::Arc<mln_text::glyph::FontFaces>>,
    pub sprite_loaded: bool,
    pub transition_options: TransitionOptions,
    pub light: Immutable<light::Impl>,
    pub images: Immutable<Vec<Immutable<image::Impl>>>,
    pub sources: ImmutableVector<dyn source::Impl>,
    pub layers: ImmutableVector<dyn layer::Impl>,
    /// The style's global state, used by "global-state" expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,

    // TODO(port): mapbox::base::WeakPtr<AnnotationManager> annotationManager.
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
}
