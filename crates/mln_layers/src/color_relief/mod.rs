//! The `color-relief` layer type: a layer of a raster-dem source that
//! colors its tiles' terrain by elevation.

pub(crate) mod render_layer;
#[rustfmt::skip]
mod properties;

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::ColorReliefPaintProperties;

/// The `color-relief` layer type.
pub struct ColorRelief;

impl StyleLayerType for ColorRelief {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "color-relief", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = ColorReliefPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
