use std::sync::Arc;

use crate::expression::Functions;
use crate::layer::{Layer, LayerTypeInfo, SourceKind};
use crate::layer_type::{LayerStyle, StyleLayerType};
use crate::layers::fill_extrusion_layer::FillExtrusionLayer;
use crate::layers::fill_extrusion_layer_properties::{
    self, FillExtrusionLayoutProperties, FillExtrusionPaintProperties,
};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `fill-extrusion` layer type.
pub struct FillExtrusion;

/// The style data of a `fill-extrusion` layer.
pub type Impl = LayerStyle<FillExtrusion>;

impl StyleLayerType for FillExtrusion {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "fill-extrusion",
        source: SourceKind::Geometry,
        pass_3d: true,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &fill_extrusion_layer_properties::PROPERTIES;

    type LayoutProperties = FillExtrusionLayoutProperties::Unevaluated;
    type PaintProperties = FillExtrusionPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_extrusion_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(a: &Impl, b: &Impl) -> bool {
        a.filter != b.filter
            || a.visibility != b.visibility
            || a.layout != b.layout
            || a.paint.has_data_driven_property_difference(&b.paint)
    }

    fn into_layer(style: Arc<Impl>, functions: Arc<Functions>) -> Option<Box<dyn Layer>> {
        Some(Box::new(FillExtrusionLayer::from_impl(style, functions)))
    }
}
