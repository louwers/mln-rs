use crate::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::layers::heatmap_layer_properties::{self, HeatmapPaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};

/// The `heatmap` layer type.
pub struct Heatmap;

impl StyleLayerType for Heatmap {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "heatmap",
        source: SourceKind::Geometry,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &heatmap_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HeatmapPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        heatmap_layer_properties::visit(slot, visitor)
    }
}
