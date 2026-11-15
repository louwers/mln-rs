use std::sync::Arc;

use super::{Convertible, Error};
use crate::expression::Functions;
use crate::light::{Impl, Light};
use crate::property_table;

/// Converts the style's `light`, whose expressions parse against `functions`.
pub fn convert_light(value: &Convertible<'_>, functions: &Arc<Functions>, error: &mut Error) -> Option<Light> {
    if !value.is_object() {
        error.message = "light must be an object".to_owned();
        return None;
    }

    let mut light = Light::new();
    for name in property_table::names::<Impl>() {
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
