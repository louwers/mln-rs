use crate::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use crate::layers::fill_layer_properties::{self, FillLayoutProperties, FillPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `fill` layer type.
pub struct Fill;

impl StyleLayerType for Fill {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "fill",
        source: SourceKind::Geometry,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &fill_layer_properties::PROPERTIES;
    // A constant pattern's image is a dependency of the layout.
    const LAYOUT_PAINT_PROPERTIES: &'static [&'static str] = &["fill-pattern"];

    type LayoutProperties = FillLayoutProperties::Unevaluated;
    type PaintProperties = FillPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_layer_properties::visit(slot, visitor)
    }
}
