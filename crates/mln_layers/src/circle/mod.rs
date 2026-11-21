//! The `circle` layer type: a layer of a geometry source that draws its
//! point features as circles.

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

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::{CircleLayoutProperties, CirclePaintProperties};

/// The `circle` layer type.
pub struct Circle;

impl StyleLayerType for Circle {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "circle", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = CircleLayoutProperties::Unevaluated;
    type PaintProperties = CirclePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
