// Ports scripts/style-spec.mjs (the edits MapLibre Native makes to the style
// spec reference) and the part of scripts/generate-style-code.mjs that turns
// the spec into the per-layer property lists.

use serde_json::{Map, Value, json};

/// One layout, paint, light or sky property: the spec entry plus its name.
#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub spec: Map<String, Value>,
    /// `property['light-property'] === true` (set for light properties).
    pub light: bool,
}

impl Property {
    pub fn type_(&self) -> &str {
        self.spec.get("type").and_then(Value::as_str).unwrap_or_else(|| panic!("{}: no type", self.name))
    }

    /// `property.value` (the element type of an array).
    pub fn value(&self) -> Option<&str> {
        self.spec.get("value").and_then(Value::as_str)
    }

    /// `property.length`.
    pub fn length(&self) -> Option<usize> {
        self.spec.get("length").and_then(Value::as_u64).map(|n| n as usize)
    }

    /// `property['property-type']`.
    pub fn property_type(&self) -> Option<&str> {
        self.spec.get("property-type").and_then(Value::as_str)
    }

    /// `property.default` (None when the key is absent, i.e. `undefined`).
    pub fn default(&self) -> Option<&Value> {
        self.spec.get("default")
    }

    pub fn period(&self) -> Option<&Value> {
        self.spec.get("period")
    }

    pub fn units(&self) -> Option<&str> {
        self.spec.get("units").and_then(Value::as_str)
    }
}

/// One entry of the `layers` array in generate-style-code.mjs.
#[derive(Clone, Debug)]
pub struct LayerSpec {
    pub type_: String,
    /// Layout properties without `visibility`, sorted.
    pub layout_properties: Vec<Property>,
    /// Paint properties, sorted (for `line` including the mock
    /// `line-floor-width`).
    pub paint_properties: Vec<Property>,
}

/// `modifiedReferenceSpec` (style-spec.mjs) with generate-style-code.mjs's
/// `line-floor-width` mock property added.
pub fn load(text: &str) -> Result<Value, String> {
    let mut spec: Value = serde_json::from_str(text).map_err(|e| format!("v8.json: {e}"))?;

    // https://github.com/maplibre/maplibre-native/issues/250
    remove(&mut spec, &["layout_symbol", "text-rotation-alignment", "values"], "viewport-glyph")?;

    // https://github.com/maplibre/maplibre-native/issues/251
    remove(&mut spec, &["layout_symbol"], "icon-overlap")?;
    remove(&mut spec, &["layout_symbol"], "text-overlap")?;
    // Quirk kept: style-spec.mjs assigns text-allow-overlap's `requires` twice,
    // the second time from icon-allow-overlap's (filtered) list.
    let filtered = |spec: &Value, property: &str, drop: &str| -> Result<Value, String> {
        let requires = spec["layout_symbol"][property]["requires"]
            .as_array()
            .ok_or_else(|| format!("layout_symbol.{property}.requires is not an array"))?;
        let drop: Value = serde_json::from_str(drop).expect("valid JSON");
        Ok(Value::Array(requires.iter().filter(|v| **v != drop).cloned().collect()))
    };
    let text_requires = filtered(&spec, "text-allow-overlap", r#"{"!":"text-overlap"}"#)?;
    spec["layout_symbol"]["text-allow-overlap"]["requires"] = text_requires;
    let icon_requires = filtered(&spec, "icon-allow-overlap", r#"{"!":"icon-overlap"}"#)?;
    spec["layout_symbol"]["text-allow-overlap"]["requires"] = icon_requires;

    // https://github.com/maplibre/maplibre-native/issues/1929
    remove(&mut spec, &["layout_symbol"], "symbol-height-anchor")?;
    remove(&mut spec, &["layout_symbol"], "symbol-height-offset")?;

    // https://github.com/maplibre/maplibre-native/issues/4298
    remove(&mut spec, &["paint_fill"], "fill-layer-opacity")?;
    remove(&mut spec, &["paint_line"], "line-layer-opacity")?;

    // https://github.com/maplibre/maplibre-native/issues/2963
    remove(&mut spec, &["paint_fill"], "fill-layer-blend")?;
    remove(&mut spec, &["paint_line"], "line-layer-blend")?;
    remove(&mut spec, &["paint_raster"], "raster-layer-blend")?;

    // https://github.com/maplibre/maplibre-native/issues/4117
    remove(&mut spec, &["paint_raster"], "resampling")?;
    remove(&mut spec, &["paint_hillshade"], "resampling")?;
    remove(&mut spec, &["paint_color-relief"], "resampling")?;

    // https://github.com/maplibre/maplibre-native/issues/4194
    zoom_only(&mut spec, "layout_line", "line-cap", "data-constant")?;
    zoom_only(&mut spec, "layout_line", "line-miter-limit", "data-constant")?;
    zoom_only(&mut spec, "layout_line", "line-round-limit", "data-constant")?;

    // https://github.com/maplibre/maplibre-native/issues/4517
    zoom_only(&mut spec, "layout_symbol", "icon-rotation-alignment", "data-constant")?;

    // https://github.com/maplibre/maplibre-native/issues/744
    zoom_only(&mut spec, "paint_line", "line-dasharray", "cross-faded")?;

    spec["layer"]["type"]["values"]["location-indicator"] = json!({});
    spec["layout_location-indicator"] = location_indicator_layout();
    spec["paint_location-indicator"] = location_indicator_paint();

    // internal use
    spec["layout_symbol"]["symbol-screen-space"] = json!({
        "type": "boolean",
        "default": false,
        "property-type": "data-constant",
        "doc": "Internal use only"
    });

    // generate-style-code.mjs: "Add this mock property that our SDF line
    // shader needs so that it gets added to the list of "data driven"
    // properties."
    spec["paint_line"]["line-floor-width"] = json!({
        "type": "number",
        "default": 1,
        "property-type": "data-driven"
    });

    Ok(spec)
}

fn remove(spec: &mut Value, path: &[&str], key: &str) -> Result<(), String> {
    let mut node = &mut *spec;
    for p in path {
        node = node.get_mut(*p).ok_or_else(|| format!("spec has no {}", path.join(".")))?;
    }
    let object = node.as_object_mut().ok_or_else(|| format!("{} is not an object", path.join(".")))?;
    object.shift_remove(key).map(|_| ()).ok_or_else(|| format!("spec has no {}.{key}", path.join(".")))
}

/// Makes a data-driven property follow zoom only, as `property_type`.
fn zoom_only(spec: &mut Value, group: &str, property: &str, property_type: &str) -> Result<(), String> {
    let entry = spec
        .get_mut(group)
        .and_then(|g| g.get_mut(property))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("spec has no {group}.{property}"))?;
    entry.insert("property-type".to_owned(), json!(property_type));
    let parameters = entry
        .get_mut("expression")
        .and_then(|e| e.get_mut("parameters"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("{group}.{property} has no expression parameters"))?;
    parameters.retain(|p| p != "feature");
    Ok(())
}

/// JSON doesn't have a defined order, so generate-style-code.mjs sorts the
/// properties with `new Intl.Collator("en-US").compare`. For names made of
/// lowercase ASCII letters, digits and '-', ICU's root collation (which en-US
/// uses; punctuation is not ignorable) orders them like their bytes: '-'
/// sorts before digits, digits before letters, and a prefix first. Other
/// characters are rejected rather than guessed.
pub fn sort_like_collator(properties: &mut [Property]) {
    for p in properties.iter() {
        assert!(
            p.name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "property name {:?} needs ICU collation, which the generator does not implement",
            p.name
        );
    }
    properties.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
}

fn properties_of(spec: &Value, key: &str, skip_visibility: bool) -> Result<Vec<Property>, String> {
    let object = spec[key].as_object().ok_or_else(|| format!("spec has no {key}"))?;
    let mut result = Vec::new();
    for (name, value) in object {
        if skip_visibility && name == "visibility" {
            continue;
        }
        let spec = value.as_object().ok_or_else(|| format!("{key}.{name} is not an object"))?.clone();
        result.push(Property { name: name.clone(), spec, light: false });
    }
    sort_like_collator(&mut result);
    Ok(result)
}

/// The `layers` array of generate-style-code.mjs, in `spec.layer.type.values`
/// order.
pub fn layers(spec: &Value) -> Result<Vec<LayerSpec>, String> {
    let types = spec["layer"]["type"]["values"].as_object().ok_or("spec has no layer.type.values")?;
    let mut result = Vec::new();
    for type_ in types.keys() {
        result.push(LayerSpec {
            type_: type_.clone(),
            layout_properties: properties_of(spec, &format!("layout_{type_}"), true)?,
            paint_properties: properties_of(spec, &format!("paint_{type_}"), false)?,
        });
    }
    Ok(result)
}

/// `lightProperties` of generate-style-code.mjs.
pub fn light_properties(spec: &Value) -> Result<Vec<Property>, String> {
    let mut result = properties_of(spec, "light", false)?;
    for p in &mut result {
        p.light = true;
    }
    sort_like_collator(&mut result);
    Ok(result)
}

/// The sky's properties, as generateRootProperties('Sky', 'sky') of
/// generate-style-code.mjs collects them.
pub fn sky_properties(spec: &Value) -> Result<Vec<Property>, String> {
    let mut result = properties_of(spec, "sky", false)?;
    sort_like_collator(&mut result);
    Ok(result)
}

fn location_indicator_layout() -> Value {
    json!({
        "top-image": {
            "type": "resolvedImage",
            "property-type": "data-constant",
            "expression": {"interpolated": false, "parameters": ["zoom"]},
            "doc": "Name of image in sprite to use as the top of the location indicator."
        },
        "bearing-image": {
            "type": "resolvedImage",
            "property-type": "data-constant",
            "expression": {"interpolated": false, "parameters": ["zoom"]},
            "doc": "Name of image in sprite to use as the middle of the location indicator."
        },
        "shadow-image": {
            "type": "resolvedImage",
            "property-type": "data-constant",
            "expression": {"interpolated": false, "parameters": ["zoom"]},
            "doc": "Name of image in sprite to use as the background of the location indicator."
        }
    })
}

fn location_indicator_paint() -> Value {
    // Quirk kept: "perspective-compensation" and "image-tilt-displacement"
    // have string defaults in style-spec.mjs.
    json!({
        "perspective-compensation": {
            "type": "number",
            "default": "0.85",
            "property-type": "data-constant",
            "expression": {"interpolated": true, "parameters": ["zoom"]}
        },
        "image-tilt-displacement": {
            "type": "number",
            "property-type": "data-constant",
            "default": "0",
            "units": "pixels",
            "expression": {"interpolated": true, "parameters": ["zoom"]}
        },
        "bearing": {
            "type": "number",
            "default": 0,
            "period": 360,
            "units": "degrees",
            "property-type": "data-constant",
            "expression": {"interpolated": false, "parameters": []},
            "transition": true
        },
        "location": {
            "type": "array",
            "default": [0.0, 0.0, 0.0],
            "length": 3,
            "value": "number",
            "property-type": "data-constant",
            "expression": {"interpolated": true, "parameters": []},
            "transition": true
        },
        "accuracy-radius": {
            "type": "number",
            "units": "meters",
            "default": 0,
            "property-type": "data-constant",
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        },
        "top-image-size": {
            "type": "number",
            "units": "factor of the original icon size",
            "property-type": "data-constant",
            "default": 1,
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        },
        "bearing-image-size": {
            "type": "number",
            "units": "factor of the original icon size",
            "property-type": "data-constant",
            "default": 1,
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        },
        "shadow-image-size": {
            "type": "number",
            "units": "factor of the original icon size",
            "property-type": "data-constant",
            "default": 1,
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        },
        "accuracy-radius-color": {
            "type": "color",
            "property-type": "data-constant",
            "default": "#ffffff",
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        },
        "accuracy-radius-border-color": {
            "type": "color",
            "property-type": "data-constant",
            "default": "#ffffff",
            "expression": {"interpolated": true, "parameters": ["zoom"]},
            "transition": true
        }
    })
}
