use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::property::table::{self, InPlace, SetPropertyError, StyleProperty};

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
    use crate::color::Color;
);

/// The style's sky.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sky {
    pub properties: SkyProperties::Transitionable,
}

impl Sky {
    #[must_use]
    pub fn new() -> Self {
        Sky::default()
    }

    /// Sets a property (or `<name>-transition`) from a style JSON value,
    /// whose expressions parse against `functions`.
    pub fn set_property(&mut self, name: &str, value: &Convertible<'_>, functions: &Arc<Functions>) -> Option<Error> {
        match table::set_property(&mut InPlace(self), name, value, functions) {
            Ok(()) => None,
            Err(SetPropertyError::Invalid(error)) => Some(error),
            Err(SetPropertyError::Unknown) => Some(Error::new("sky doesn't support this property")),
        }
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        table::get_property(self, name)
    }
}

/// Converts the style's `sky`, whose expressions parse against `functions`.
pub fn convert_sky(value: &Convertible<'_>, functions: &Arc<Functions>, error: &mut Error) -> Option<Sky> {
    if !value.is_object() {
        error.message = "sky must be an object".to_owned();
        return None;
    }

    let mut sky = Sky::new();
    for name in table::names::<Sky>() {
        let Some(member) = value.object_member(&name) else {
            continue;
        };
        if let Some(conversion_error) = sky.set_property(&name, &member, functions) {
            *error = conversion_error;
            return None;
        }
    }

    Some(sky)
}
