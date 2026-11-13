use mln_conversion::Convertible;
use mln_map_types::CameraOptions;
use mln_style::Style;

use crate::runner::MapError;

/// Loads the style from `json`. Errors reach the map's events, as in C++.
pub fn load_json(style: &mut Style, json: &str) {
    style.load_json(json);
}

pub fn load_url(style: &mut Style, url: &str) {
    style.load_url(url);
}

#[must_use]
pub fn default_camera(style: &Style) -> CameraOptions {
    style.get_default_camera()
}

/// Sets the paint or layout property `property_name` of the layer
/// `layer_name`. The runner ignores an error from setting the property.
pub fn set_layer_property(
    style: &mut Style,
    layer_name: &str,
    property_name: &str,
    value: &serde_json::Value,
) -> Result<(), MapError> {
    let Some(mut layer) = style.get_layer_mut(layer_name) else {
        return Err(MapError::LayerNotFound);
    };
    let _ = layer.set_property(property_name, &Convertible::from(value), None);
    Ok(())
}

/// Sets the zoom range of the layer `layer_name`.
pub fn set_layer_zoom_range(style: &mut Style, layer_name: &str, min_zoom: f32, max_zoom: f32) -> Result<(), MapError> {
    let Some(mut layer) = style.get_layer_mut(layer_name) else {
        return Err(MapError::LayerNotFound);
    };
    layer.set_min_zoom(min_zoom);
    layer.set_max_zoom(max_zoom);
    Ok(())
}

/// Parses a layer from `json` and adds it to the style.
pub fn add_layer(style: &mut Style, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_conversion::Error::default();
    let Some(layer) = mln_style::conversion::layer::convert_layer(&Convertible::from(&value), &mut error) else {
        return Err(MapError::Conversion(error.message));
    };
    // C++ Style::addLayer throws (uncaught by the runner) on a duplicate id.
    style.add_layer(layer, None).map_err(|e| MapError::Exception(e.to_string()))
}

/// Converts `filter` (a conversion error first) and sets it on the layer
/// `layer_name` (`LayerNotFound` if there is none).
pub fn set_layer_filter(style: &mut Style, layer_name: &str, filter: &serde_json::Value) -> Result<(), MapError> {
    let mut error = mln_conversion::Error::default();
    let Some(filter) =
        mln_style::conversion::convert::<mln_style::filter::Filter>(&Convertible::from(filter), &mut error)
    else {
        return Err(MapError::Conversion(error.message));
    };
    let Some(mut layer) = style.get_layer_mut(layer_name) else {
        return Err(MapError::LayerNotFound);
    };
    layer.set_filter(&filter);
    Ok(())
}

/// Sets the global state property `name`. Its error (style not loaded) is
/// a [`MapError::Exception`], as the C++ runner doesn't catch it.
pub fn set_global_state_property(style: &mut Style, name: &str, value: &feature::Value) -> Result<(), MapError> {
    style.set_global_state_property(name, value).map_err(|e| MapError::Exception(e.to_string()))
}

/// Removes the layer `layer_name`, and returns whether there was one.
pub fn remove_layer(style: &mut Style, layer_name: &str) -> bool {
    style.remove_layer(layer_name).is_some()
}

/// Parses a source named `source_name` from `json` and adds it to the
/// style. A duplicate id is a [`MapError::Exception`], as C++ throws an
/// exception the runner doesn't catch.
pub fn add_source(style: &mut Style, source_name: &str, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_conversion::Error::default();
    let Some(source) =
        mln_style::conversion::source::convert_source(&Convertible::from(&value), &mut error, source_name)
    else {
        return Err(MapError::Conversion(error.message));
    };
    style.add_source(source).map_err(|e| MapError::Exception(e.to_string()))
}

/// Parses a light from `json` and sets it on the style (for the conformance
/// suite).
pub fn set_light(style: &mut Style, json: &str) -> Result<(), MapError> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(e) => return Err(MapError::Conversion(e.to_string())),
    };
    let mut error = mln_conversion::Error::default();
    let Some(light) = mln_style::conversion::convert::<mln_style::light::Light>(&Convertible::from(&value), &mut error)
    else {
        return Err(MapError::Conversion(error.message));
    };
    style.set_light(light);
    Ok(())
}

/// Removes the source `source_name`, and returns whether there was one.
pub fn remove_source(style: &mut Style, source_name: &str) -> bool {
    style.remove_source(source_name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_json(id: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/metrics/integration/render-tests")
            .join(id)
            .join("style.json");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn transition_test_style() {
        let mut style = Style::new(1.0);
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
        let mut style = Style::new(1.0);
        load_json(&mut style, &style_json("background-color/colorSpace-hcl"));
        assert_eq!(default_camera(&style).zoom, Some(5.0));
    }

    #[test]
    fn layers() {
        let mut style = Style::new(1.0);
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
