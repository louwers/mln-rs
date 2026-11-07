// Ports include/mln/style/conversion/projection.hpp +
// src/mln/style/conversion/projection.cpp.

use mln_style_types::ProjectionDefinition;

use super::{Converter, Convertible, Error, convert_property_value};
use crate::projection::Projection;

impl Converter for Projection {
    fn convert(value: &Convertible<'_>, error: &mut Error) -> Option<Projection> {
        if !value.is_object() {
            error.message = "projection must be an object".to_owned();
            return None;
        }

        let mut projection = Projection::new();

        if let Some(type_) = value.object_member("type") {
            let converted = convert_property_value::<ProjectionDefinition>(&type_, error, false, false)?;
            projection.set_type(converted);
        }

        Some(projection)
    }
}
