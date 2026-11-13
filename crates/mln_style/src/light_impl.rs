// `LightProperty<T>` has the same typedefs as `PaintProperty<T>`, so the tags
// use the `paint_property!` macro.

use mln_style_types::{LightAnchorType, Position};
use mln_util::Color;

crate::paint_property!(pub struct LightAnchor: LightAnchorType = LightAnchorType::Viewport);

crate::paint_property!(pub struct LightPosition: Position = Position::new([1.15_f32, 210.0_f32, 30.0_f32]));

crate::paint_property!(pub struct LightColor: Color = Color::white());

crate::paint_property!(pub struct LightIntensity: f32 = 0.5_f32);

crate::paint_properties!(
    #[expect(non_snake_case, reason = "named for the C++ `LightProperties` type list")]
    pub mod LightProperties {
        light_anchor: LightAnchor,
        light_position: LightPosition,
        light_color: LightColor,
        light_intensity: LightIntensity,
    }
);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub properties: LightProperties::Transitionable,
}
