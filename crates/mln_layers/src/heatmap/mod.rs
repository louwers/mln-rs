//! The `heatmap` layer type: a layer of a geometry source that draws the
//! density of its point features as a color ramp.

pub(crate) mod bucket;
pub(crate) mod layout;
pub(crate) mod render_layer;
#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::{HeatmapPaintProperties, default_heatmap_color};

/// The `heatmap` layer type.
pub struct Heatmap;

impl StyleLayerType for Heatmap {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "heatmap", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = HeatmapPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
