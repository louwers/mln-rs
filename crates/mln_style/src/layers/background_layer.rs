use crate::layer::{LayerTypeInfo, SourceKind};
use crate::layer_type::{NoProperties, StyleLayerType};
pub use crate::layers::background_layer_impl::Impl;
use crate::layers::background_layer_properties::{self, BackgroundPaintProperties};
use crate::property_table::{PropertyTable, PropertyVisitor};

/// The `background` layer type.
pub struct Background;

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
}
