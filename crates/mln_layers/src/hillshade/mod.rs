//! The `hillshade` layer type: a layer of a raster-dem source that shades
//! its tiles' terrain as lit from one or more directions.

mod layer;
#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::HillshadeLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::HillshadePaintProperties;

/// The `hillshade` layer type's style half.
pub struct Hillshade;

impl StyleLayerType for Hillshade {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "hillshade", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HillshadePaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
