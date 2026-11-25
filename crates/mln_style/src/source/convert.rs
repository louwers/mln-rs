use crate::StyleRegistry;
use crate::conversion::{Convertible, Error};
use crate::source::Source;

/// Converts the source `id` through its type in `registry`. A GeoJSON
/// source's `clusterProperties` expressions parse against the registry's
/// functions.
pub fn convert_source(
    value: &Convertible<'_>,
    error: &mut Error,
    id: &str,
    registry: &StyleRegistry,
) -> Option<Source> {
    if !value.is_object() {
        error.message = "source must be an object".to_owned();
        return None;
    }

    let Some(type_value) = value.object_member("type") else {
        error.message = "source must have a type".to_owned();
        return None;
    };

    let Some(type_) = type_value.to_string() else {
        error.message = "source type must be a string".to_owned();
        return None;
    };
    if let Some(source_type) = registry.source_type(&type_).filter(|source_type| source_type.info().from_json) {
        source_type.create_from_json(id, value, registry.functions(), error)
    } else {
        error.message = "invalid source type".to_owned();
        None
    }
}
