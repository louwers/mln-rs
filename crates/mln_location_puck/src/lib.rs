#![doc = include_str!("../README.md")]
//!
//! # The crate's parts
//!
//! - the style half, here: [`LocationPuck`] and its [`paint`] properties,
//!   and [`register`];
//! - the parts it draws and where, on the ground around the position
//!   (`parts.rs`);
//! - the program, with its block and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`), which draws each part's shape in its quad;
//! - the render half, [`LocationPuckLayer`], which pushes a draw of each
//!   part in each world copy and hit-tests the puck and the arrow
//!   (`layer.rs`).

mod glsl;
mod glsl_es;
mod layer;
mod msl;
mod parts;
mod program;
mod wgsl;

pub use layer::LocationPuckLayer;
use mln_rs::layer::{
    Color, LayerStyle, LayerTypeInfo, NoProperties, PropertyTable, PropertyVisitor, SourceKind, StyleLayerType,
};
use mln_rs::mln_render::{RegisterError, RegistryBuilder};

mln_rs::layer::properties!(
    /// The paint properties of an `org.maplibre.location-puck` layer.
    pub mod paint: paint {
        position: constant LatLng = LatLng::default(),
        bearing: constant Rotation = Rotation::default(),
        bearing_visible: constant bool = false,
        bearing_radius: constant f32 = 18.0,
        bearing_arrow_color: constant Color = Color::white(),
        bearing_accuracy: constant f32 = 0.0,
        bearing_accuracy_radius: constant f32 = 64.0,
        bearing_accuracy_color: constant Color = blue(0.3),
        accuracy_radius: constant f32 = 0.0,
        accuracy_color: constant Color = blue(0.15),
        accuracy_border_width: constant f32 = 0.0,
        accuracy_border_color: constant Color = blue(0.4),
        puck_radius: constant f32 = 8.0,
        puck_color: constant Color = blue(1.0),
        puck_border_width: constant f32 = 2.0,
        puck_border_color: constant Color = Color::white(),
        shadow_radius: constant f32 = 0.0,
        shadow_color: constant Color = Color::new(0.0, 0.0, 0.0, 0.25),
        perspective_compensation: constant f32 = 0.85,
        tilt_displacement: constant f32 = 0.0,
    }
    use crate::blue, mln_rs::layer::Color, mln_rs::layer::LatLng, mln_rs::layer::Rotation;
);

/// The puck's blue, `#2b8af0`, at `alpha`.
fn blue(alpha: f32) -> Color {
    let [r, g, b] = [43.0, 138.0, 240.0].map(|channel: f32| channel / 255.0 * alpha);
    Color::new(r, g, b, alpha)
}

/// The `org.maplibre.location-puck` layer type's style half.
pub struct LocationPuck;

impl StyleLayerType for LocationPuck {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.location-puck", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &paint::PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        paint::visit(slot, visitor)
    }
}

/// Adds the `org.maplibre.location-puck` layer type to `builder`.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<LocationPuckLayer>()
}
