//! The `fill-extrusion` layer type: a layer of a geometry source that
//! extrudes its polygon features into 3D shapes.

pub(crate) mod bucket;
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

pub use layer::FillExtrusionLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};

/// The `fill-extrusion` layer type's style half.
pub struct FillExtrusion;

impl StyleLayerType for FillExtrusion {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "fill-extrusion", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = properties::layout::Unevaluated;
    type PaintProperties = properties::paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
