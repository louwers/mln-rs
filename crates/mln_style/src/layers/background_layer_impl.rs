use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use crate::layers::background_layer::BackgroundLayer;
use crate::layers::background_layer_properties::{self, BackgroundPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `background` layer type.
pub struct Background;

/// The style data of a `background` layer.
pub type Impl = LayerStyle<Background>;

impl StyleLayerType for Background {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "background",
        source: SourceKind::None,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &background_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = BackgroundPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        background_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(BackgroundLayer::from_impl(style, functions)))
    }
}
