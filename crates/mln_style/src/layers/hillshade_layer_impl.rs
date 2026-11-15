use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use crate::layers::hillshade_layer::HillshadeLayer;
use crate::layers::hillshade_layer_properties::{self, HillshadePaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `hillshade` layer type.
pub struct Hillshade;

/// The style data of a `hillshade` layer.
pub type Impl = LayerStyle<Hillshade>;

impl StyleLayerType for Hillshade {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "hillshade",
        source: SourceKind::RasterDem,
        pass_3d: true,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &hillshade_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HillshadePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        hillshade_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(HillshadeLayer::from_impl(style, functions)))
    }
}
