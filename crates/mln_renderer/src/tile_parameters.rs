use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{SchedulerPtr, TaggedScheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_text::glyph::FontFaces;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::tile::tile_worker_transport::TileWorkerTransport;
use crate::tile::tile_workers::SourceCounters;

pub(crate) struct TileParameters<'a> {
    pub(crate) pixel_ratio: f32,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) transform_state: &'a TransformState,
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    /// The renderer's loop: tiles take their responses and replies there.
    pub(crate) scheduler: SchedulerPtr,
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
    /// The counters of the source whose tiles these parameters make.
    pub(crate) source: Rc<SourceCounters>,
    // symbols:
    /// The images that exist, which tiles lay out for.
    pub(crate) available_images: Arc<VecSet<String>>,
    /// The style's font faces, which the tiles' workers shape text with.
    pub(crate) font_faces: Option<Arc<FontFaces>>,
    /// Port-only: creates the workers of raster and raster-dem tiles
    /// (`tile_worker_transport.rs`).
    pub(crate) tile_worker_transport: Rc<dyn TileWorkerTransport>,
    /// Port-only: the orchestrator's workers, which free the data of the
    /// tiles' layouts (see `render_orchestrator::drop_on_workers`).
    pub(crate) thread_pool: TaggedScheduler,
}
