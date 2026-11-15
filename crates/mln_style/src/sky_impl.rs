crate::properties!(
    #[expect(non_snake_case, reason = "named like the layers' property sets")]
    pub mod SkyProperties: paint {
        sky_atmosphere_blend: constant f32 = 0.8_f32,
        sky_fog_color: constant Color = Color::white(),
        sky_fog_ground_blend: constant f32 = 0.5_f32,
        sky_horizon_color: constant Color = Color::white(),
        sky_horizon_fog_blend: constant f32 = 0.8_f32,
        sky_color: constant Color =
            Color::new(136.0_f32 / 255.0_f32, 198.0_f32 / 255.0_f32, 252.0_f32 / 255.0_f32, 1.0_f32),
        sky_horizon_blend: constant f32 = 0.8_f32,
    }
    use mln_util::Color;
);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub properties: SkyProperties::Transitionable,
}
