use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, StyleLayerType};
use crate::layers::circle_layer::CircleLayer;
use crate::layers::circle_layer_properties::{self, CircleLayoutProperties, CirclePaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `circle` layer type.
pub struct Circle;

/// The style data of a `circle` layer.
pub type Impl = LayerStyle<Circle>;

impl StyleLayerType for Circle {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "circle",
        source: SourceKind::Geometry,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &circle_layer_properties::PROPERTIES;

    type LayoutProperties = CircleLayoutProperties::Unevaluated;
    type PaintProperties = CirclePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        circle_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(a: &Impl, b: &Impl) -> bool {
        a.filter != b.filter
            || a.visibility != b.visibility
            || a.layout != b.layout
            || a.paint.has_data_driven_property_difference(&b.paint)
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(CircleLayer::from_impl(style, functions)))
    }
}
