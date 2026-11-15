use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::StyleLayerType;
pub use crate::layers::fill_layer_impl::Impl;
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

    type LayoutProperties = FillLayoutProperties::Unevaluated;
    type PaintProperties = FillPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        fill_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(a: &Impl, b: &Impl) -> bool {
        a.filter != b.filter
            || a.visibility != b.visibility
            || a.layout != b.layout
            || a.paint.fill_pattern.value != b.paint.fill_pattern.value
            || a.paint.has_data_driven_property_difference(&b.paint)
    }
}
