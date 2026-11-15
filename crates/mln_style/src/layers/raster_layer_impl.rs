use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use crate::layers::raster_layer::RasterLayer;
use crate::layers::raster_layer_properties::{self, RasterPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `raster` layer type.
pub struct Raster;

/// The style data of a `raster` layer.
pub type Impl = LayerStyle<Raster>;

impl StyleLayerType for Raster {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "raster",
        source: SourceKind::Raster,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &raster_layer_properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = RasterPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        raster_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(_a: &Impl, _b: &Impl) -> bool {
        false
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(RasterLayer::from_impl(style, functions)))
    }
}
