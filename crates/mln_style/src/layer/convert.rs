use super::{Convertible, Error};
use crate::layer::{Layer, PropertyScope, SourceKind};
use crate::log_target;
use crate::member::{Place, is_extension_member};
use crate::registry::StyleRegistry;

fn set_object_member(layer: &mut Layer, value: &Convertible<'_>, member: &str, error: &mut Error) -> bool {
    if let Some(member_value) = value.object_member(member)
        && let Some(error_) = layer.set_property(member, &member_value, None)
    {
        *error = error_;
        return false;
    }
    true
}

/// What converting a layer does with a layout or paint property that it can't
/// set: the layer doesn't take the property, or the value doesn't fit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvalidProperty {
    /// The conversion fails with the property's error.
    Fail,
    /// The conversion warns with the property's error and leaves the property
    /// unset, as a style load does.
    Skip,
}

/// Sets each member of the object `properties` (the layer's `layout` or
/// `paint`) as a property of `layer`.
fn set_properties(
    layer: &mut Layer,
    properties: &Convertible<'_>,
    scope: PropertyScope,
    invalid: InvalidProperty,
) -> Option<Error> {
    properties.each_member(|name, value| {
        let error = layer.set_property(name, value, Some(scope))?;
        match invalid {
            InvalidProperty::Fail => Some(error),
            InvalidProperty::Skip => {
                log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
                None
            }
        }
    })
}

pub(crate) fn set_paint_properties(
    layer: &mut Layer,
    value: &Convertible<'_>,
    invalid: InvalidProperty,
) -> Option<Error> {
    let paint_value = value.object_member("paint")?;
    if !paint_value.is_object() {
        return Some(Error::new(format!("layer '{}': paint must be an object", layer.get_id())));
    }
    set_properties(layer, &paint_value, PropertyScope::Paint, invalid)
}

/// Converts a [layer](https://maplibre.org/maplibre-style-spec/layers/) of
/// one of `registry`'s types, whose expressions parse against its functions.
/// Fails on the first error, including a layout or paint property that the
/// layer can't set.
pub fn convert_layer(registry: &StyleRegistry, value: &Convertible<'_>, error: &mut Error) -> Option<Layer> {
    convert_layer_with(registry, value, error, InvalidProperty::Fail)
}

pub(crate) fn convert_layer_with(
    registry: &StyleRegistry,
    value: &Convertible<'_>,
    error: &mut Error,
    invalid: InvalidProperty,
) -> Option<Layer> {
    if !value.is_object() {
        error.message = "layer must be an object".to_owned();
        return None;
    }

    let Some(id_value) = value.object_member("id") else {
        error.message = "layer must have an id".to_owned();
        return None;
    };

    let Some(id) = id_value.to_string() else {
        error.message = "layer id must be a string".to_owned();
        return None;
    };

    let Some(type_value) = value.object_member("type") else {
        error.message = "layer must have a type".to_owned();
        return None;
    };

    let Some(type_) = type_value.to_string() else {
        error.message = "layer type must be a string".to_owned();
        return None;
    };

    let mut layer = registry.create_layer(&type_, &id, value, error)?;

    if !set_object_member(&mut layer, value, "minzoom", error) {
        return None;
    }
    if !set_object_member(&mut layer, value, "maxzoom", error) {
        return None;
    }
    if !set_object_member(&mut layer, value, "filter", error) {
        return None;
    }
    if layer.info().source != SourceKind::None && !set_object_member(&mut layer, value, "source-layer", error) {
        return None;
    }

    if let Some(layout_value) = value.object_member("layout") {
        if !layout_value.is_object() {
            error.message = format!("layer '{id}': layout must be an object");
            return None;
        }
        if let Some(error_) = set_properties(&mut layer, &layout_value, PropertyScope::Layout, invalid) {
            *error = error_;
            return None;
        }
    }

    if let Some(error_) = set_paint_properties(&mut layer, value, invalid) {
        *error = error_;
        return None;
    }

    if let Some(error_) = set_members(registry, &mut layer, value, invalid) {
        *error = error_;
        return None;
    }

    Some(layer)
}

/// Sets the extension members of `layer` from the members of its style JSON
/// `value` whose names are those of extension members.
fn set_members(
    registry: &StyleRegistry,
    layer: &mut Layer,
    value: &Convertible<'_>,
    invalid: InvalidProperty,
) -> Option<Error> {
    value.each_member(|name, member| {
        if !is_extension_member(name) {
            return None;
        }
        match registry.member_value(Place::Layer, name, member) {
            Ok(value) => {
                layer.set_member(name, value);
                None
            }
            Err(error) => {
                let error = Error::new(format!("layer '{}': {}", layer.get_id(), error.message));
                match invalid {
                    InvalidProperty::Fail => Some(error),
                    InvalidProperty::Skip => {
                        log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
                        None
                    }
                }
            }
        }
    })
}
