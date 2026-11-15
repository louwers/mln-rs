use std::sync::Arc;

use crate::layer::{LayerTypeInfo, layer_type_info};
pub use crate::layers::background_layer_impl::Impl;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "background",
    source: layer_type_info::Source::NotRequired,
    pass3d: layer_type_info::Pass3D::NotRequired,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::NotRequired,
};

impl Impl {
    #[must_use]
    pub fn static_type_info() -> &'static LayerTypeInfo {
        &TYPE_INFO
    }
}

crate::layer::layer_type!(
    /// A `background` layer.
    BackgroundLayer
);

impl BackgroundLayer {
    #[must_use]
    pub fn new(layer_id: &str) -> Self {
        BackgroundLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), String::new())))
    }
}
