use super::{Converter, Convertible, Error};
use crate::property_table;
use crate::sky::{Impl, Sky};

impl Converter for Sky {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Sky> {
        if !value.is_object() {
            error.message = "sky must be an object".to_owned();
            return None;
        }

        let mut sky = Sky::new();
        for name in property_table::names::<Impl>() {
            let Some(member) = value.object_member(&name) else {
                continue;
            };
            if let Some(conversion_error) = sky.set_property(&name, &member) {
                *error = conversion_error;
                return None;
            }
        }

        Some(sky)
    }
}
