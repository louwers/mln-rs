use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_tile_id::OverscaledTileID;

#[derive(Clone, Debug)]
pub(crate) struct BucketParameters {
    pub(crate) tile_id: OverscaledTileID,
    pub(crate) mode: MapMode,
    pub(crate) pixel_ratio: f32,
    /// The style's global state, used by "global-state" expressions.
    pub(crate) global_state: Option<Arc<GlobalStateMap>>,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
}
