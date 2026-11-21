//! The `hillshade` layer type: a layer of a raster-dem source that shades
//! its tiles' terrain as lit from one or more directions.

pub(crate) mod render_layer;
#[rustfmt::skip]
mod properties;
pub mod programs;
pub mod ubo;
crate::shaders::language_modules!();

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::HillshadePaintProperties;

/// The `hillshade` layer type.
pub struct Hillshade;

impl StyleLayerType for Hillshade {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "hillshade", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HillshadePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
