//! The `fill-extrusion` layer type: a layer of a geometry source that
//! extrudes its polygon features into 3D shapes.

pub(crate) mod bucket;
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
pub use properties::{FillExtrusionLayoutProperties, FillExtrusionPaintProperties};

/// The `fill-extrusion` layer type.
pub struct FillExtrusion;

impl StyleLayerType for FillExtrusion {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "fill-extrusion", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = FillExtrusionLayoutProperties::Unevaluated;
    type PaintProperties = FillExtrusionPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
