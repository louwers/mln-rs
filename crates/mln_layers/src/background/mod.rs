//! The `background` layer type: a layer without a source that fills the
//! map with a color or a pattern.

#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod msl;
mod pattern_atlas;
pub mod programs;
pub(crate) mod render_layer;
pub mod ubo;
mod wgsl;

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::BackgroundPaintProperties;

/// The `background` layer type.
pub struct Background;

impl StyleLayerType for Background {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "background", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = BackgroundPaintProperties::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
