//! The `line` layer type: a layer of a geometry source that strokes its
//! line and polygon features.

pub(crate) mod bucket;
pub(crate) mod dash_array_binder;
mod dashes;
pub(crate) mod mesh;
pub(crate) mod render_layer;
#[rustfmt::skip]
mod properties;
pub mod programs;
pub mod ubo;
crate::shaders::language_modules!();

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::{LineLayoutProperties, LinePaintProperties};

/// The `line` layer type.
pub struct Line;

impl StyleLayerType for Line {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "line", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = LineLayoutProperties::Unevaluated;
    type PaintProperties = LinePaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
