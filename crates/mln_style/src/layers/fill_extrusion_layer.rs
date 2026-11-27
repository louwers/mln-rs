use crate::layer::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::fill_extrusion_layer_properties::{
    self, FillExtrusionLayoutProperties, FillExtrusionPaintProperties,
};
use crate::property::table::{PropertyTable, PropertyVisitor};

/// The `fill-extrusion` layer type.
pub struct FillExtrusion;

impl StyleLayerType for FillExtrusion {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "fill-extrusion", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &fill_extrusion_layer_properties::PROPERTIES;

    type LayoutProperties = FillExtrusionLayoutProperties::Unevaluated;
    type PaintProperties = FillExtrusionPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_extrusion_layer_properties::visit(slot, visitor)
    }
}
