use crate::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::fill_layer_properties::{self, FillLayoutProperties, FillPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `fill` layer type.
pub struct Fill;

impl StyleLayerType for Fill {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "fill", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &fill_layer_properties::PROPERTIES;

    type LayoutProperties = FillLayoutProperties::Unevaluated;
    type PaintProperties = FillPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_layer_properties::visit(slot, visitor)
    }
}
