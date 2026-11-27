//! The `org.maplibre.ngon` layer type: a regular polygon, of 3 to 360
//! corners, at each point of a geometry source, with a circle layer's paint
//! properties named `ngon-*` and two more, `ngon-corners` and
//! `ngon-rotate`. [`register`] adds it to a registry.
//!
//! The style half is [`Ngon`] and its [`paint`] properties. The layout
//! makes a bucket of a quad per point (`bucket.rs`), and the render half
//! draws each tile's quads with its program (`program.rs`, with its text in
//! each shading language) and hit-tests the polygons (`layer.rs`).

mod bucket;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
mod layer;
#[rustfmt::skip]
mod msl;
mod program;
mod wgsl;

use mln_engine::RegistryBuilder;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use crate::layer::NgonLayer;

mln_style::layer_properties!(
    /// The paint properties of an `org.maplibre.ngon` layer.
    pub mod paint: paint {
        ngon_radius: data_driven f32 = 5.0,
        ngon_corners: data_driven f32 = 5.0,
        ngon_rotate: data_driven f32 = 0.0,
        ngon_color: data_driven Color = Color::black(),
        ngon_blur: data_driven f32 = 0.0,
        ngon_opacity: data_driven f32 = 1.0,
        ngon_stroke_width: data_driven f32 = 0.0,
        ngon_stroke_color: data_driven Color = Color::black(),
        ngon_stroke_opacity: data_driven f32 = 1.0,
        ngon_translate: constant [f32; 2] = [0.0, 0.0],
        ngon_translate_anchor: constant TranslateAnchorType = TranslateAnchorType::Map,
        ngon_pitch_alignment: constant AlignmentType = AlignmentType::Viewport,
        ngon_pitch_scale: constant CirclePitchScaleType = CirclePitchScaleType::Map,
    }
    use mln_style::color::Color, mln_style::types::AlignmentType, mln_style::types::CirclePitchScaleType,
        mln_style::types::TranslateAnchorType;
);

/// The `org.maplibre.ngon` layer type's style half.
pub struct Ngon;

impl StyleLayerType for Ngon {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.ngon", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        visit(slot, visitor)
    }
}

/// Adds the `org.maplibre.ngon` layer type to `builder`. Fails if the
/// builder has a layer type of that name.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<NgonLayer>()
}
