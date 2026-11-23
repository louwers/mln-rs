//! The `heatmap` layer type: a layer of a geometry source that draws the
//! density of its point features as a color ramp.

mod bucket;
mod layer;
mod layout;
#[rustfmt::skip]
pub mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::HeatmapLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::default_heatmap_color;

/// The `heatmap` layer type's style half.
pub struct Heatmap;

impl StyleLayerType for Heatmap {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "heatmap", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = properties::paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
