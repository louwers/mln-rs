// Ports src/mln/style/sky_impl.hpp + src/mln/style/sky_impl.cpp.
//
// `SkyProperty<T>` has the same typedefs as `PaintProperty<T>`, so the tags
// use the `paint_property!` macro, as the light's do.

use mln_util::Color;

crate::paint_property!(pub struct SkyAtmosphereBlend: f32 = 0.8_f32);

crate::paint_property!(pub struct SkyFogColor: Color = Color::white());

crate::paint_property!(pub struct SkyFogGroundBlend: f32 = 0.5_f32);

crate::paint_property!(pub struct SkyHorizonColor: Color = Color::white());

crate::paint_property!(pub struct SkyHorizonFogBlend: f32 = 0.8_f32);

crate::paint_property!(pub struct SkyColor: Color = Color::new(136.0_f32 / 255.0_f32, 198.0_f32 / 255.0_f32, 252.0_f32 / 255.0_f32, 1.0_f32));

crate::paint_property!(pub struct SkyHorizonBlend: f32 = 0.8_f32);

crate::paint_properties!(
    /// `using SkyProperties = Properties<SkyAtmosphereBlend, SkyFogColor,
    /// SkyFogGroundBlend, SkyHorizonColor, SkyHorizonFogBlend, SkyColor,
    /// SkyHorizonBlend>`.
    #[allow(non_snake_case)]
    pub mod SkyProperties {
        sky_atmosphere_blend: SkyAtmosphereBlend,
        sky_fog_color: SkyFogColor,
        sky_fog_ground_blend: SkyFogGroundBlend,
        sky_horizon_color: SkyHorizonColor,
        sky_horizon_fog_blend: SkyHorizonFogBlend,
        sky_color: SkyColor,
        sky_horizon_blend: SkyHorizonBlend,
    }
);

/// `Sky::Impl`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Impl {
    pub properties: SkyProperties::Transitionable,
}
