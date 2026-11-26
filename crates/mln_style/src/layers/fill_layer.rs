use crate::layer::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::fill_layer_properties::{self, FillLayoutProperties, FillPaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};
use crate::types::LayerBlendType;

/// The `fill` layer type.
pub struct Fill;

impl StyleLayerType for Fill {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "fill",
        source: SourceKind::Geometry,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &fill_layer_properties::PROPERTIES;

    type LayoutProperties = FillLayoutProperties::Unevaluated;
    type PaintProperties = FillPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_layer_properties::visit(slot, visitor)
    }

    fn layer_blend(paint: &FillPaintProperties::PossiblyEvaluated) -> LayerBlendType {
        *paint.fill_layer_blend.uniform()
    }
}
