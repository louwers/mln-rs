use std::sync::Arc;

use super::{Convertible, Error};
use crate::expression::Functions;
use crate::light::Light;
use crate::property::table;

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
