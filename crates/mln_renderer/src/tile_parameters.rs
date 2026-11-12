// Ports src/mln/renderer/tile_parameters.hpp.

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::TaggedScheduler;
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlasPtr;
use crate::image_manager::ImageManager;
use crate::text::glyph_manager::GlyphManager;
use crate::tile::tile_worker_transport::TileWorkerTransport;

/// `TileParameters`.
// TODO(port): annotationManager.
pub struct TileParameters<'a> {
    pub pixel_ratio: f32,
    pub debug_options: MapDebugOptions,
    pub transform_state: &'a TransformState,
    pub file_source: Option<Arc<dyn FileSource>>,
    pub mode: MapMode,
    pub prefetch_zoom_delta: u8,
    pub thread_pool: TaggedScheduler,
    pub tile_lod_min_radius: f64,
    pub tile_lod_scale: f64,
    pub tile_lod_pitch_threshold: f64,
    pub tile_lod_zoom_shift: f64,
    pub tile_lod_mode: TileLodMode,
    pub subdivision_granularity: SubdivisionGranularitySetting,
    pub global_state: Option<Arc<GlobalStateMap>>,
    pub is_update_synchronous: bool,
    pub capture_rendered_features: bool,
    // symbols:
    pub image_manager: Rc<ImageManager>,
    pub glyph_manager: Rc<GlyphManager>,
    pub dynamic_texture_atlas: DynamicTextureAtlasPtr,
    /// Port-only: creates the tiles' workers (`tile_worker_transport.rs`).
    pub tile_worker_transport: Rc<dyn TileWorkerTransport>,
}
