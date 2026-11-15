use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use crate::layers::color_relief_layer::ColorReliefLayer;
use crate::layers::color_relief_layer_properties::{self, ColorReliefPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `color-relief` layer type.
pub struct ColorRelief;

/// The style data of a `color-relief` layer.
pub type Impl = LayerStyle<ColorRelief>;

impl StyleLayerType for ColorRelief {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "color-relief",
        source: SourceKind::RasterDem,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &color_relief_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = ColorReliefPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        color_relief_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(ColorReliefLayer::from_impl(style, functions)))
    }
}
