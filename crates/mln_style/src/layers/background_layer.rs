use crate::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::layers::background_layer_properties::{self, BackgroundPaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};

/// The `background` layer type.
pub struct Background;

impl StyleLayerType for Background {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "background", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &background_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = BackgroundPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        background_layer_properties::visit(slot, visitor)
    }
}
