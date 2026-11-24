use std::sync::Arc;

use mln_style_types::ProjectionDefinition;

use super::{Convertible, Error};
use crate::expression::Functions;
use crate::projection::Projection;
use crate::property::convert::convert_property_value;

/// Converts the style's `projection`, whose expressions parse against
/// `functions`.
pub fn convert_projection(
    value: &Convertible<'_>,
    functions: &Arc<Functions>,
    error: &mut Error,
) -> Option<Projection> {
    if !value.is_object() {
        error.message = "projection must be an object".to_owned();
        return None;
    }

    let mut projection = Projection::new();

    if let Some(type_) = value.object_member("type") {
        let converted = convert_property_value::<ProjectionDefinition>(&type_, error, false, false, functions)?;
        projection.set_type(converted);
    }

    Some(projection)
}
