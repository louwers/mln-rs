//! The `fill` layer type: a layer of a geometry source that fills its
//! polygon features with a color or a pattern, and can outline them.

pub(crate) mod bucket;
pub(crate) mod mesh;
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
pub use properties::{FillLayoutProperties, FillPaintProperties};

/// The `fill` layer type.
pub struct Fill;

impl StyleLayerType for Fill {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "fill", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = FillLayoutProperties::Unevaluated;
    type PaintProperties = FillPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
