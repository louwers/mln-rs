use mln_engine::camera::CameraOptions;
use mln_engine::map::Map;
use mln_style::conversion::Convertible;
use mln_style::error::ErrorKind;
use mln_style::{Edit, Style};

use crate::runner::MapError;

#[must_use]
pub(crate) fn default_camera(style: &Style) -> CameraOptions {
    mln_engine::map::style_camera(style)
}

/// Sets the paint or layout property `property_name` of the layer
/// `layer_name`. The runner ignores an error from setting the property.
pub(crate) fn set_layer_property(
    style: &mut Style,
    layer_name: &str,
    property_name: &str,
    value: &serde_json::Value,
) -> Result<(), MapError> {
    if style.get_layer(layer_name).is_none() {
        return Err(MapError::LayerNotFound);
    }
    let edit =
        Edit::SetLayerProperty { layer: layer_name.to_owned(), name: property_name.to_owned(), value: value.clone() };
    let _ = style.apply(edit);
    Ok(())
}

/// Sets the zoom range of the layer `layer_name`.
pub(crate) fn set_layer_zoom_range(
    style: &mut Style,
    layer_name: &str,
    min_zoom: f32,
    max_zoom: f32,
) -> Result<(), MapError> {
    let edit = Edit::SetLayerZoomRange { layer: layer_name.to_owned(), min: min_zoom, max: max_zoom };
    style.apply(edit).map_err(|_| MapError::LayerNotFound)
}

/// Parses a layer from `json` and adds it to the style.
pub(crate) fn add_layer(style: &mut Style, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_style::conversion::Error::default();
    let Some(layer) = mln_style::layer::convert_layer(style.registry(), &Convertible::from(&value), &mut error) else {
        return Err(MapError::Conversion(error.message));
    };
    // C++ Style::addLayer throws (uncaught by the runner) on a duplicate id.
    style.apply(Edit::AddLayer { layer, before: None }).map_err(|e| MapError::Exception(e.to_string()))
}

/// Converts `filter` (a conversion error first) and sets it on the layer
/// `layer_name` (`LayerNotFound` if there is none).
pub(crate) fn set_layer_filter(
    style: &mut Style,
    layer_name: &str,
    filter: &serde_json::Value,
) -> Result<(), MapError> {
    let mut error = mln_style::conversion::Error::default();
    let Some(filter) =
        mln_style::filter::convert_filter(&Convertible::from(filter), style.registry().functions(), &mut error)
    else {
        return Err(MapError::Conversion(error.message));
    };
    let edit = Edit::SetLayerFilter { layer: layer_name.to_owned(), filter };
    style.apply(edit).map_err(|_| MapError::LayerNotFound)
}

/// Sets the global state property `name`. Its error (style not loaded) is
/// a [`MapError::Exception`], as the C++ runner doesn't catch it.
pub(crate) fn set_global_state_property(map: &mut Map, name: &str, value: &feature::Value) -> Result<(), MapError> {
    let edit = Edit::SetGlobalState { name: name.to_owned(), value: value.clone() };
    map.edit_loaded_style(|style| style.apply(edit)).map_err(|e| MapError::Exception(e.to_string()))
}

/// Removes the layer `layer_name`, and returns whether there was one.
pub(crate) fn remove_layer(style: &mut Style, layer_name: &str) -> bool {
    style.apply(Edit::RemoveLayer(layer_name.to_owned())).is_ok()
}

/// Parses a source named `source_name` from `json` and adds it to the
/// map's style. A duplicate id is a [`MapError::Exception`], as C++ throws
/// an exception the runner doesn't catch.
pub(crate) fn add_source(map: &mut Map, source_name: &str, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_style::conversion::Error::default();
    let registry = map.style().registry();
    let Some(source) = mln_style::source::convert_source(&Convertible::from(&value), &mut error, source_name, registry)
    else {
        return Err(MapError::Conversion(error.message));
    };
    map.add_source(source).map_err(|e| MapError::Exception(e.to_string()))
}

/// Parses a light from `json` and sets it on the style (for the conformance
/// suite).
pub(crate) fn set_light(style: &mut Style, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_style::conversion::Error::default();
    let Some(light) =
        mln_style::light::convert_light(&Convertible::from(&value), style.registry().functions(), &mut error)
    else {
        return Err(MapError::Conversion(error.message));
    };
    let _ = style.apply(Edit::SetLight(light));
    Ok(())
}

/// Sets the style's root `terrain` from `json`, or removes it if `json` is
/// `null`. A style that doesn't take terrain is [`MapError::Unsupported`].
pub(crate) fn set_terrain(style: &mut Style, json: &str) -> Result<(), MapError> {
    style.set_root_property_json("terrain", json).map_err(|e| match e.kind {
        ErrorKind::Unsupported => MapError::Unsupported(e.message),
        _ => MapError::Conversion(e.message),
    })
}

/// Removes the source `source_name`, and returns whether there was one.
pub(crate) fn remove_source(map: &mut Map, source_name: &str) -> bool {
    map.remove_source(source_name).is_some()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mln_engine::Registry;

    use super::*;

    fn style_json(id: &str) -> String {
        let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/metrics/integration/render-tests")
            .join(id)
            .join("style.json");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn load_json(style: &mut Style, json: &str) {
        *style = Style::from_json(style.registry(), json).expect("a style");
    }

    #[test]
    fn transition_test_style() {
        let mut style = Style::new(Arc::clone(Registry::with_builtins().style()));
        load_json(&mut style, &style_json("background-color/transition"));
        assert!(style.get_layer("background").is_some());
        assert_eq!(
            set_layer_property(&mut style, "background", "background-color", &serde_json::json!("white")),
            Ok(())
        );
        assert_eq!(
            set_layer_property(&mut style, "nope", "background-color", &serde_json::json!("white")),
            Err(MapError::LayerNotFound)
        );
    }

    #[test]
    fn default_camera_of_a_test_style() {
        let mut style = Style::new(Arc::clone(Registry::with_builtins().style()));
        load_json(&mut style, &style_json("background-color/colorSpace-hcl"));
        assert_eq!(default_camera(&style).zoom, Some(5.0));
    }

    #[test]
    fn layers() {
        let mut style = Style::new(Arc::clone(Registry::with_builtins().style()));
        load_json(&mut style, &style_json("background-visibility/none"));
        assert_eq!(set_layer_zoom_range(&mut style, "background", 1.0, 2.0), Ok(()));
        assert_eq!(set_layer_zoom_range(&mut style, "nope", 1.0, 2.0), Err(MapError::LayerNotFound));
        assert!(remove_layer(&mut style, "background"));
        assert!(!remove_layer(&mut style, "background"));
        assert_eq!(add_layer(&mut style, r#"{"id": "b2", "type": "background"}"#), Ok(()));
        assert!(style.get_layer("b2").is_some());
        assert!(matches!(add_layer(&mut style, r#"{"type": "background"}"#), Err(MapError::Conversion(_))));
    }
}
