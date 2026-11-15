use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{LayerTypeInfo, layer_type_info};
pub use crate::layers::line_layer_impl::Impl;

static TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "line",
    source: layer_type_info::Source::Required,
    pass3d: layer_type_info::Pass3D::NotRequired,
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
    /// A `line` layer.
    LineLayer
);

impl LineLayer {
    /// A layer whose property values parse against the built-in expression
    /// functions.
    #[must_use]
    pub fn new(layer_id: &str, source_id: &str) -> Self {
        LineLayer::with_functions(Functions::with_builtins(), layer_id, source_id)
    }

    /// A layer whose property values parse against `functions`.
    #[must_use]
    pub(crate) fn with_functions(functions: Arc<Functions>, layer_id: &str, source_id: &str) -> Self {
        LineLayer::from_impl(Arc::new(Impl::new(layer_id.to_owned(), source_id.to_owned())), functions)
    }
}
