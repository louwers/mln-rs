#![doc = include_str!("../README.md")]
//!
//! # The crate's parts
//!
//! - the style half, here: [`Ngon`], its [`paint`] and [`layout`]
//!   properties, and [`register`];
//! - the layout, which turns a tile's points into a bucket of quads with
//!   the values of the data-driven properties (`bucket.rs`);
//! - the program, with its blocks and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`);
//! - the render half, [`NgonLayer`], which makes a mesh of each tile's
//!   bucket, pushes its draws and hit-tests the polygons (`layer.rs`);
//! - placement, [`NgonPlacement`], which shows each polygon of a layer that
//!   collides where its circle meets nothing placed before it
//!   (`placement.rs`).

mod bucket;
mod glsl;
mod glsl_es;
mod layer;
mod msl;
mod placement;
mod program;
mod wgsl;

pub use layer::NgonLayer;
use mln_rs::layer::{
    LayerStyle, LayerTypeInfo, PropertyInfo, PropertyTable, PropertyVisitor, SourceKind, StyleLayerType,
};
use mln_rs::mln_render::{RegisterError, RegistryBuilder};
pub use placement::NgonPlacement;

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

mln_rs::layer::properties!(
    /// The layout properties of an `org.maplibre.ngon` layer: how its
    /// polygons collide with what else places. A layer whose two are both
    /// `true`, as they are by default, doesn't place.
    pub mod layout: layout {
        ngon_allow_overlap: constant bool = true,
        ngon_ignore_placement: constant bool = true,
    }
);

/// The paint properties, in `paint`'s order, then the layout properties.
static PROPERTIES: PropertyTable = PropertyTable::unsorted(&[
    PropertyInfo::paint("ngon-radius").data_driven(),
    PropertyInfo::paint("ngon-corners").data_driven(),
    PropertyInfo::paint("ngon-rotate").data_driven(),
    PropertyInfo::paint("ngon-color").data_driven(),
    PropertyInfo::paint("ngon-blur").data_driven(),
    PropertyInfo::paint("ngon-opacity").data_driven(),
    PropertyInfo::paint("ngon-stroke-width").data_driven(),
    PropertyInfo::paint("ngon-stroke-color").data_driven(),
    PropertyInfo::paint("ngon-stroke-opacity").data_driven(),
    PropertyInfo::paint("ngon-translate"),
    PropertyInfo::paint("ngon-translate-anchor"),
    PropertyInfo::paint("ngon-pitch-alignment"),
    PropertyInfo::paint("ngon-pitch-scale"),
    PropertyInfo::layout("ngon-allow-overlap"),
    PropertyInfo::layout("ngon-ignore-placement"),
]);

/// The `org.maplibre.ngon` layer type's style half.
pub struct Ngon;

impl StyleLayerType for Ngon {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.ngon", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = layout::Unevaluated;
    type PaintProperties = paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        match slot {
            13 => Some(visitor.layout(|s| &s.layout.ngon_allow_overlap, |s| &mut s.layout.ngon_allow_overlap)),
            14 => Some(visitor.layout(|s| &s.layout.ngon_ignore_placement, |s| &mut s.layout.ngon_ignore_placement)),
            _ => paint::visit(slot, visitor),
        }
    }
}

/// Adds the `org.maplibre.ngon` layer type to `builder`, with its
/// placement.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_placing_layer_type::<NgonPlacement>()
}
