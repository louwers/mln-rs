use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::StyleLayerType;
pub use crate::layers::line_layer_impl::Impl;
use crate::layers::line_layer_properties::{self, LineLayoutProperties, LinePaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `line` layer type.
pub struct Line;

impl StyleLayerType for Line {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "line",
        source: SourceKind::Geometry,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: true,
    };
    const PROPERTIES: &'static PropertyTable = &line_layer_properties::PROPERTIES;

    type LayoutProperties = LineLayoutProperties::Unevaluated;
    type PaintProperties = LinePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<Impl>>(slot: usize, visitor: V) -> Option<V::Output> {
        line_layer_properties::visit(slot, visitor)
    }

    fn layout_difference(a: &Impl, b: &Impl) -> bool {
        a.filter != b.filter
            || a.visibility != b.visibility
            || a.layout != b.layout
            || a.paint.has_data_driven_property_difference(&b.paint)
    }
}
