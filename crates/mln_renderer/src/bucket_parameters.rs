use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_tile_id::OverscaledTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

/// The tile that a layout lays out, and how.
#[derive(Clone, Debug)]
pub struct BucketParameters {
    pub tile_id: OverscaledTileID,
    pub mode: MapMode,
    pub pixel_ratio: f32,
    /// The style's global state, used by "global-state" expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,
    /// How finely the globe projection subdivides the tile's geometry.
    pub subdivision_granularity: SubdivisionGranularitySetting,
}
