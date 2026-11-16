use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::{NoProperties, StyleLayerType};
pub use crate::layers::heatmap_layer_impl::Impl;
use crate::layers::heatmap_layer_properties::{self, HeatmapPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `heatmap` layer type.
pub struct Heatmap;

impl StyleLayerType for Heatmap {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "heatmap",
        source: SourceKind::Geometry,
        pass_3d: true,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &heatmap_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HeatmapPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        heatmap_layer_properties::visit(slot, visitor)
    }
}
