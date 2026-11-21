//! The `background` layer type: a layer without a source that fills the
//! map with a color or a pattern.

#[rustfmt::skip]
mod properties;
pub mod programs;
pub mod ubo;
crate::shaders::shader_sources!();
mod pattern_atlas;
pub(crate) mod render_layer;

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
