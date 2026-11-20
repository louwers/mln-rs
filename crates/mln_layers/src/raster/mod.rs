//! The `raster` layer type: a layer of a raster or image source that draws
//! its tiles' images or the source's image.

pub(crate) mod render_layer;
#[rustfmt::skip]
mod properties;
pub mod programs;
pub mod ubo;
crate::shaders::shader_sources!();

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::RasterPaintProperties;

/// The `raster` layer type.
pub struct Raster;

impl StyleLayerType for Raster {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "raster", source: SourceKind::Raster, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = RasterPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
