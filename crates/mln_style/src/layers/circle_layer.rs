use crate::layer::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::circle_layer_properties::{self, CircleLayoutProperties, CirclePaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};

/// The `circle` layer type.
pub struct Circle;

impl StyleLayerType for Circle {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "circle",
        source: SourceKind::Geometry,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &circle_layer_properties::PROPERTIES;

    type LayoutProperties = CircleLayoutProperties::Unevaluated;
    type PaintProperties = CirclePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        circle_layer_properties::visit(slot, visitor)
    }
}
