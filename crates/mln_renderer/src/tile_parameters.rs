use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlasPtr;
use crate::image_manager::ImageManager;
use crate::text::glyph_manager::GlyphManager;
use crate::tile::tile_worker_transport::TileWorkerTransport;

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
    pub(crate) capture_rendered_features: bool,
    // symbols:
    pub(crate) image_manager: Rc<ImageManager>,
    pub(crate) glyph_manager: Rc<GlyphManager>,
    pub(crate) dynamic_texture_atlas: DynamicTextureAtlasPtr,
    /// Port-only: creates the tiles' workers (`tile_worker_transport.rs`).
    pub(crate) tile_worker_transport: Rc<dyn TileWorkerTransport>,
}
