use crate::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::layers::hillshade_layer_properties::{self, HillshadePaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};

/// The `hillshade` layer type.
pub struct Hillshade;

impl StyleLayerType for Hillshade {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "hillshade",
        source: SourceKind::RasterDem,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &hillshade_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HillshadePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        hillshade_layer_properties::visit(slot, visitor)
    }
}
