use crate::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::layers::raster_layer_properties::{self, RasterPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `raster` layer type.
pub struct Raster;

impl StyleLayerType for Raster {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "raster", source: SourceKind::Raster, from_json: true };
    const PROPERTIES: &'static PropertyTable = &raster_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = RasterPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        raster_layer_properties::visit(slot, visitor)
    }
}
