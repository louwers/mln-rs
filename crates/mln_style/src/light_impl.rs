crate::properties!(
    #[expect(non_snake_case, reason = "named like the layers' property sets")]
    pub mod LightProperties: paint {
        light_anchor: constant LightAnchorType = LightAnchorType::Viewport,
        light_position: constant Position = Position::new([1.15_f32, 210.0_f32, 30.0_f32]),
        light_color: constant Color = Color::white(),
        light_intensity: constant f32 = 0.5_f32,
    }
    use mln_style_types::LightAnchorType, mln_style_types::Position, mln_util::Color;
);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub properties: LightProperties::Transitionable,
}
