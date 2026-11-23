//! The `color-relief` layer type: a layer of a raster-dem source that
//! colors its tiles' terrain by elevation.

mod layer;
#[rustfmt::skip]
pub mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::ColorReliefLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};

/// The `color-relief` layer type's style half.
pub struct ColorRelief;

impl StyleLayerType for ColorRelief {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "color-relief", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = properties::paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
