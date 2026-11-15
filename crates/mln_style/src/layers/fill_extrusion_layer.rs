use std::sync::Arc;

use crate::layer::{LayerTypeInfo, layer_type_info};
pub use crate::layers::fill_extrusion_layer_impl::Impl;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "fill-extrusion",
    source: layer_type_info::Source::Required,
    pass3d: layer_type_info::Pass3D::Required,
    layout: layer_type_info::Layout::Required,
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
    /// A `fill-extrusion` layer.
    FillExtrusionLayer
);

impl FillExtrusionLayer {
    #[must_use]
    pub fn new(layer_id: &str, source_id: &str) -> Self {
        FillExtrusionLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())))
    }
}
