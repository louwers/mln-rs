//! The `line` layer type: a layer of a geometry source that strokes its
//! line and polygon features.

pub(crate) mod bucket;
mod dashes;
mod layer;
mod layout;
pub(crate) mod mesh;
#[rustfmt::skip]
pub mod properties;
mod glsl;
mod glsl_es;
mod msl;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::LineLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};

/// The `line` layer type's style half.
pub struct Line;

impl StyleLayerType for Line {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "line", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = properties::layout::Unevaluated;
    type PaintProperties = properties::paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
