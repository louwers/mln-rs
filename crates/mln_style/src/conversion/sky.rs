use std::sync::Arc;

use super::{Convertible, Error};
use crate::expression::Functions;
use crate::property::table;
use crate::sky::Sky;

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
