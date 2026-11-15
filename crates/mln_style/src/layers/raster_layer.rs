use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::{NoProperties, StyleLayerType};
pub use crate::layers::raster_layer_impl::Impl;
use crate::layers::raster_layer_properties::{self, RasterPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `raster` layer type.
pub struct Raster;

impl StyleLayerType for Raster {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "raster",
        source: SourceKind::Raster,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &raster_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = RasterPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        raster_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }
}
