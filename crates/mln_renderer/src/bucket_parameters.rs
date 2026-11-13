use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_style::layer::LayerTypeInfo;
use mln_tile_id::OverscaledTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

#[derive(Clone, Debug)]
pub(crate) struct BucketParameters {
    pub(crate) tile_id: OverscaledTileID,
    pub(crate) mode: MapMode,
    pub(crate) pixel_ratio: f32,
    pub(crate) layer_type: &'static LayerTypeInfo,
    /// The style's global state, used by "global-state" expressions.
    pub(crate) global_state: Option<Arc<GlobalStateMap>>,
    pub(crate) retain_features_by_id: bool,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
}
