use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::{NoProperties, StyleLayerType};
pub use crate::layers::color_relief_layer_impl::Impl;
use crate::layers::color_relief_layer_properties::{self, ColorReliefPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `color-relief` layer type.
pub struct ColorRelief;

impl StyleLayerType for ColorRelief {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "color-relief",
        source: SourceKind::RasterDem,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &color_relief_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = ColorReliefPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        color_relief_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }
}
