use super::{Convertible, Error};
use crate::layer::{Layer, PropertyScope, layer_type_info};
use crate::layermanager::LayerManager;

fn set_object_member(layer: &mut Box<dyn Layer>, value: &Convertible<'_>, member: &str, error: &mut Error) -> bool {
    if let Some(member_value) = value.object_member(member)
        && let Some(error_) = layer.set_property(member, &member_value, None)
    {
        *error = error_;
        return false;
    }
    true
}

pub(crate) fn set_paint_properties(layer: &mut dyn Layer, value: &Convertible<'_>) -> Option<Error> {
    let paint_value = value.object_member("paint")?;
    if !paint_value.is_object() {
        return Some(Error::new(format!("layer '{}': paint must be an object", layer.get_id())));
    }
    paint_value.each_member(|k, v| layer.set_property(k, v, Some(PropertyScope::Paint)))
}

pub fn convert_layer(value: &Convertible<'_>, error: &mut Error) -> Option<Box<dyn Layer>> {
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

    let mut layer = LayerManager::get().create_layer(&type_, &id, value, error)?;

    if !set_object_member(&mut layer, value, "minzoom", error) {
        return None;
    }
    if !set_object_member(&mut layer, value, "maxzoom", error) {
        return None;
    }
    if !set_object_member(&mut layer, value, "filter", error) {
        return None;
    }
    if layer.get_type_info().source == layer_type_info::Source::Required
        && !set_object_member(&mut layer, value, "source-layer", error)
    {
        return None;
    }

    if let Some(layout_value) = value.object_member("layout") {
        if !layout_value.is_object() {
            error.message = format!("layer '{id}': layout must be an object");
            return None;
        }
        let error_ = layout_value.each_member(|k, v| layer.set_property(k, v, Some(PropertyScope::Layout)));
        if let Some(error_) = error_ {
            *error = error_;
            return None;
        }
    }

    if let Some(error_) = set_paint_properties(layer.as_mut(), value) {
        *error = error_;
        return None;
    }

    Some(layer)
}
