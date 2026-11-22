//! The `circle` layer type: a layer of a geometry source that draws its
//! point features as circles.

mod bucket;
mod layer;
mod layout;
#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::CircleLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::{CircleLayoutProperties, CirclePaintProperties};

/// The `circle` layer type's style half.
pub struct Circle;

impl StyleLayerType for Circle {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "circle", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = CircleLayoutProperties::Unevaluated;
    type PaintProperties = CirclePaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
