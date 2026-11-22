//! The `background` layer type: a layer without a source that fills the
//! map with a color or a pattern.

#[rustfmt::skip]
mod properties;
mod glsl;
mod glsl_es;
mod layer;
mod msl;
mod pattern_atlas;
pub mod programs;
pub mod ubo;
mod wgsl;

pub use layer::BackgroundLayer;
use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
pub use properties::BackgroundPaintProperties;

/// The `background` layer type's style half.
pub struct Background;

impl StyleLayerType for Background {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "background", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &properties::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = BackgroundPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        properties::visit(slot, visitor)
    }
}
