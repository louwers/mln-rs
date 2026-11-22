//! The `raster` layer type: a layer of a raster or image source that draws
//! its tiles' images or the source's image.

mod layer;
#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::RasterLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::RasterPaintProperties;

/// The `raster` layer type's style half.
pub struct Raster;

impl StyleLayerType for Raster {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "raster", source: SourceKind::Raster, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = RasterPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
