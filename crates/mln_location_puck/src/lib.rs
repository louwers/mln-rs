//! The `org.maplibre.location-puck` layer type: a layer without a source
//! that draws a device's location at one geographic position, with its
//! heading and its accuracy. A caller adds it to its registry with
//! [`register`]; an app moves it by setting its paint properties, which
//! transition.
//!
//! It is written against the `mln_rs` facade alone, as a layer type outside
//! mln-rs is, and its modules are the parts of one:
//!
//! - the style half, here: [`LocationPuck`] and its [`paint`] properties;
//! - the parts it draws and where, on the ground around the position
//!   (`parts.rs`);
//! - the program, with its block and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`), which draws each part's shape in its quad;
//! - the render half, [`LocationPuckLayer`], which pushes a draw of each
//!   part in each world copy and hit-tests the puck and the arrow
//!   (`layer.rs`).
//!
//! # Properties
//!
//! Sizes are logical pixels but for `accuracy-radius`. A size of 0 hides
//! its part. The parts draw from the bottom up: the accuracy circle, the
//! bearing's sector, the shadow, the arrow and the puck.
//!
//! | property | default | |
//! |---|---|---|
//! | `position` | `[0, 0]` | `[longitude, latitude]` |
//! | `bearing` | 0 | the heading, degrees clockwise from north |
//! | `bearing-visible` | `false` | whether the arrow and the sector show |
//! | `bearing-radius`, `bearing-arrow-color` | 18, white | the arrow |
//! | `bearing-accuracy` | 0 | the sector's half width, degrees |
//! | `bearing-accuracy-radius`, `bearing-accuracy-color` | 64, translucent blue | the sector, which fades outward |
//! | `accuracy-radius` | 0 | the accuracy circle's radius, meters |
//! | `accuracy-color` | translucent blue | its fill |
//! | `accuracy-border-width`, `accuracy-border-color` | 0, translucent blue | its border |
//! | `puck-radius`, `puck-color` | 8, blue | the puck's fill |
//! | `puck-border-width`, `puck-border-color` | 2, white | its border, outside the fill |
//! | `shadow-radius`, `shadow-color` | 0, translucent black | a soft shadow |
//! | `perspective-compensation` | 0.85 | from 0, the puck shrinks with distance as the map does, to 1, it keeps its size |
//! | `tilt-displacement` | 0 | under pitch, how far the puck and arrow lift up the screen and the shadow drops, pixels a radian |
//!
//! Every part lies on the ground, so pitch foreshortens it, and the puck,
//! the arrow and the shadow turn with the bearing. A rendered-feature query
//! that hits the puck or the arrow returns a point at the position.

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
