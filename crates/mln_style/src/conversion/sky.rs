// Ports include/mln/style/conversion/sky.hpp +
// src/mln/style/conversion/sky.cpp.

use super::{Converter, Convertible, Error};
use crate::sky::Sky;

impl Converter for Sky {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Sky> {
        if !value.is_object() {
            error.message = "sky must be an object".to_owned();
            return None;
        }

        const PROPERTIES: [&str; 14] = [
            "atmosphere-blend",
            "atmosphere-blend-transition",
            "fog-color",
            "fog-color-transition",
            "fog-ground-blend",
            "fog-ground-blend-transition",
            "horizon-color",
            "horizon-color-transition",
            "horizon-fog-blend",
            "horizon-fog-blend-transition",
            "sky-color",
            "sky-color-transition",
            "sky-horizon-blend",
            "sky-horizon-blend-transition",
        ];

        let mut sky = Sky::new();
        for name in PROPERTIES {
            let Some(member) = value.object_member(name) else {
                continue;
            };

            if let Some(conversion_error) = sky.set_property(name, &member) {
                *error = conversion_error;
                return None;
            }
        }

        Some(sky)
    }
}
