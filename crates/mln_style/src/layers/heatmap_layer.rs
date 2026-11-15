use std::sync::Arc;

use crate::layer::{LayerTypeInfo, layer_type_info};
pub use crate::layers::heatmap_layer_impl::Impl;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "heatmap",
    source: layer_type_info::Source::Required,
    pass3d: layer_type_info::Pass3D::Required,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::Geometry,
};

impl Impl {
    #[must_use]
    pub fn static_type_info() -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

crate::layer::layer_type!(
    /// A `heatmap` layer.
    HeatmapLayer
);

impl HeatmapLayer {
    #[must_use]
    pub fn new(layer_id: &str, source_id: &str) -> Self {
        HeatmapLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())))
    }
}
