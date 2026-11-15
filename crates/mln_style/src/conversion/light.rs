use super::{Converter, Convertible, Error};
use crate::light::{Impl, Light};
use crate::property_table;

impl Converter for Light {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Light> {
        if !value.is_object() {
            error.message = "light must be an object".to_owned();
            return None;
        }

        let mut light = Light::new();
        for name in property_table::names::<Impl>() {
            let Some(member) = value.object_member(&name) else {
                continue;
            };
            if let Some(conversion_error) = light.set_property(&name, &member) {
                *error = conversion_error;
                return None;
            }
        }

        Some(light)
    }
}
