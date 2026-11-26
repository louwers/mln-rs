use crate::layer::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::line_layer_properties::{self, LineLayoutProperties, LinePaintProperties};
use crate::property::table::{PropertyTable, PropertyVisitor};
use crate::types::LayerBlendType;

/// The `line` layer type.
pub struct Line;

impl StyleLayerType for Line {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "line",
        source: SourceKind::Geometry,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &line_layer_properties::PROPERTIES;

    type LayoutProperties = LineLayoutProperties::Unevaluated;
    type PaintProperties = LinePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        line_layer_properties::visit(slot, visitor)
    }

    fn layer_blend(paint: &LinePaintProperties::PossiblyEvaluated) -> LayerBlendType {
        *paint.line_layer_blend.uniform()
    }
}
