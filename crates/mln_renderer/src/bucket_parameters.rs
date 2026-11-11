// Ports src/mln/renderer/bucket_parameters.hpp and bucket_parameters.cpp

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_style::layer::LayerTypeInfo;
use mln_tile_id::OverscaledTileID;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

/// `BucketParameters`.
#[derive(Clone, Debug)]
pub struct BucketParameters {
    pub tile_id: OverscaledTileID,
    pub mode: MapMode,
    pub pixel_ratio: f32,
    pub layer_type: &'static LayerTypeInfo,
    /// The style's global state, used by "global-state" expressions.
    pub global_state: Option<Arc<GlobalStateMap>>,
    pub retain_features_by_id: bool,
    pub subdivision_granularity: SubdivisionGranularitySetting,
}
