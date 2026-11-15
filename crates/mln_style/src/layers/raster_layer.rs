use std::sync::Arc;

use crate::layer::{LayerTypeInfo, layer_type_info};
pub use crate::layers::raster_layer_impl::Impl;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "raster",
    source: layer_type_info::Source::Required,
    pass3d: layer_type_info::Pass3D::NotRequired,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::Raster,
};

impl Impl {
    #[must_use]
    pub fn static_type_info() -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

crate::layer::layer_type!(
    /// A `raster` layer.
    RasterLayer
);

impl RasterLayer {
    #[must_use]
    pub fn new(layer_id: &str, source_id: &str) -> Self {
        RasterLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())))
    }
}
