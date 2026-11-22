#![doc = include_str!("../README.md")]
//!
//! # The crate's parts
//!
//! - the style half, here: [`Ngon`] and its [`paint`] properties, and
//!   [`register`];
//! - the layout, which turns a tile's points into a bucket of quads with
//!   the values of the data-driven properties (`layout.rs`);
//! - the program, with its blocks and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`);
//! - the render half, [`NgonLayer`], which makes a mesh of each tile's
//!   bucket, pushes its draws and hit-tests the polygons (`layer.rs`).

mod glsl;
mod glsl_es;
mod layer;
mod layout;
mod msl;
mod program;
mod wgsl;

pub use layer::NgonLayer;
use mln_rs::layer::{
    LayerStyle, LayerTypeInfo, NoProperties, PropertyTable, PropertyVisitor, SourceKind, StyleLayerType,
};
use mln_rs::mln_render::{RegisterError, RegistryBuilder};

mln_rs::layer::properties!(
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
    use mln_rs::mln_util::Color, mln_rs::mln_style_types::AlignmentType,
        mln_rs::mln_style_types::CirclePitchScaleType, mln_rs::mln_style_types::TranslateAnchorType;
);

/// The `org.maplibre.ngon` layer type's style half.
pub struct Ngon;

impl StyleLayerType for Ngon {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.ngon", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &paint::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        paint::visit(slot, visitor)
    }
}

/// Adds the `org.maplibre.ngon` layer type to `builder`.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<NgonLayer>()
}
