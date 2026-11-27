//! The `org.maplibre.location-puck` layer type: a device's location, drawn
//! at one geographic position as a puck with a border, an arrow and a
//! sector for its heading, a circle for its accuracy in meters, and a
//! shadow, with paint properties named `puck-*`. [`register`] adds it to a
//! registry.
//!
//! The style half is [`LocationPuck`] and its [`paint`] properties. Where
//! the position is in each world copy the frame shows is its anchors
//! (`anchor.rs`), the parts it shows and where they lie on the ground
//! around an anchor are `parts.rs`, and the render half draws each part's
//! shape over a quad with its program (`program.rs`, with its text in each
//! shading language) and hit-tests the puck and the arrow (`layer.rs`).

mod anchor;
#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
mod layer;
#[rustfmt::skip]
mod msl;
mod parts;
mod program;
mod wgsl;

use mln_engine::RegistryBuilder;
use mln_style::color::Color;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use crate::layer::LocationPuckLayer;

mln_style::layer_properties!(
    /// The paint properties of an `org.maplibre.location-puck` layer.
    pub mod paint: paint {
        puck_position: constant LatLng = LatLng::default(),
        puck_bearing: constant Rotation = Rotation::default(),
        puck_bearing_visible: constant bool = false,
        puck_bearing_radius: constant f32 = 18.0,
        puck_bearing_arrow_color: constant Color = Color::white(),
        puck_bearing_accuracy: constant f32 = 0.0,
        puck_bearing_accuracy_radius: constant f32 = 64.0,
        puck_bearing_accuracy_color: constant Color = blue(0.3),
        puck_accuracy_radius: constant f32 = 0.0,
        puck_accuracy_color: constant Color = blue(0.15),
        puck_accuracy_border_width: constant f32 = 0.0,
        puck_accuracy_border_color: constant Color = blue(0.4),
        puck_radius: constant f32 = 8.0,
        puck_color: constant Color = blue(1.0),
        puck_border_width: constant f32 = 2.0,
        puck_border_color: constant Color = Color::white(),
        puck_shadow_radius: constant f32 = 0.0,
        puck_shadow_color: constant Color = Color::new(0.0, 0.0, 0.0, 0.25),
        puck_perspective_compensation: constant f32 = 0.85,
        puck_tilt_displacement: constant f32 = 0.0,
    }
    use crate::blue, mln_style::color::Color, mln_style::geo::LatLng, mln_style::types::Rotation;
);

/// The puck's blue, `#2b8af0`, at `alpha`, premultiplied.
fn blue(alpha: f32) -> Color {
    let [r, g, b] = [43.0, 138.0, 240.0].map(|channel: f32| channel / 255.0 * alpha);
    Color::new(r, g, b, alpha)
}

/// The `org.maplibre.location-puck` layer type's style half.
pub struct LocationPuck;

impl StyleLayerType for LocationPuck {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.location-puck", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = paint::Transitionable;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        visit(slot, visitor)
    }
}

/// Adds the `org.maplibre.location-puck` layer type to `builder`. Fails if
/// the builder has a layer type of that name.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_layer_type::<LocationPuckLayer>()
}
