use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::property::table::{self, InPlace, OwnerSlot, SetPropertyError, StyleProperty};

crate::properties!(
    #[expect(non_snake_case, reason = "named like the layers' property sets")]
    pub mod LightProperties: paint {
        light_anchor: constant LightAnchorType = LightAnchorType::Viewport,
        light_position: constant Position = Position::new([1.15_f32, 210.0_f32, 30.0_f32]),
        light_color: constant Color = Color::white(),
        light_intensity: constant f32 = 0.5_f32,
    }
    use crate::types::LightAnchorType, crate::types::Position, crate::color::Color;
);

/// The style's light.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Light {
    pub properties: LightProperties::Transitionable,
}

impl Light {
    #[must_use]
    pub fn new() -> Self {
        Light::default()
    }

    /// Sets a property (or `<name>-transition`) from a style JSON value,
    /// whose expressions parse against `functions`.
    pub fn set_property(&mut self, name: &str, value: &Convertible<'_>, functions: &Arc<Functions>) -> Option<Error> {
        set_property(&mut InPlace(self), name, value, functions)
    }

    /// As [`set_property`](Self::set_property), on a shared light, which is
    /// copied only if the value changes it.
    pub fn set_shared_property(
        light: &mut Arc<Light>,
        name: &str,
        value: &Convertible<'_>,
        functions: &Arc<Functions>,
    ) -> Option<Error> {
        set_property(light, name, value, functions)
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        table::get_property(self, name)
    }
}

fn set_property<O: OwnerSlot<Light>>(
    owner: &mut O,
    name: &str,
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
) -> Option<Error> {
    match table::set_property(owner, name, value, functions) {
        Ok(()) => None,
        Err(SetPropertyError::Invalid(error)) => Some(error),
        Err(SetPropertyError::Unknown) => Some(Error::new("light doesn't support this property")),
    }
}

/// Converts the style's `light`, whose expressions parse against `functions`.
pub fn convert_light(value: &Convertible<'_>, functions: &Arc<Functions>, error: &mut Error) -> Option<Light> {
    if !value.is_object() {
        error.message = "light must be an object".to_owned();
        return None;
    }

    let mut light = Light::new();
    for name in table::names::<Light>() {
        let Some(member) = value.object_member(&name) else {
            continue;
        };
        if let Some(conversion_error) = light.set_property(&name, &member, functions) {
            *error = conversion_error;
            return None;
        }
    }

    Some(light)
}
