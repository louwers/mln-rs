use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use crate::layers::heatmap_layer::HeatmapLayer;
use crate::layers::heatmap_layer_properties::{self, HeatmapPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `heatmap` layer type.
pub struct Heatmap;

/// The style data of a `heatmap` layer.
pub type Impl = LayerStyle<Heatmap>;

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

    fn layout_difference(a: &Impl, b: &Impl) -> bool {
        a.filter != b.filter || a.visibility != b.visibility || a.paint.has_data_driven_property_difference(&b.paint)
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(HeatmapLayer::from_impl(style, functions)))
    }
}
