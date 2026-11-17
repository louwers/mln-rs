//! The Liberty style, changed only as far as it takes to read the snapshot.

use std::collections::BTreeSet;

use serde_json::{Value, json};

/// The source whose tiles the snapshot holds.
pub(crate) const VECTOR_SOURCE: &str = "openmaptiles";

/// Points the style at the snapshot's files, through `asset://` URLs that a
/// map's asset directory resolves: the vector source at the street tiles,
/// which a scenario swaps for another tile set; the sprite at `sprite_path`;
/// the glyphs under `glyphs/`. Removes every other source and the layers
/// that draw them, and returns a note on each removal.
pub(crate) fn localize(style: &mut Value, sprite_path: &str) -> Result<Vec<String>, String> {
    let sources = style["sources"].as_object_mut().ok_or("the style has no sources")?;
    let mut removed = Vec::new();
    for (name, source) in sources.iter() {
        if name != VECTOR_SOURCE {
            removed.push((name.clone(), source.clone()));
        }
    }
    sources.retain(|name, _| name == VECTOR_SOURCE);
    let vector = sources.get_mut(VECTOR_SOURCE).ok_or("the style has no openmaptiles source")?;
    *vector = json!({ "type": "vector", "url": "pmtiles://asset://tiles/street.pmtiles" });
    style["sprite"] = json!(format!("asset://{sprite_path}"));
    style["glyphs"] = json!("asset://glyphs/{fontstack}/{range}.pbf");

    let layers = style["layers"].as_array_mut().ok_or("the style has no layers")?;
    let mut notes = Vec::new();
    for (name, source) in removed {
        let ids: Vec<String> = layers
            .iter()
            .filter(|layer| layer["source"] == name.as_str())
            .filter_map(|layer| layer["id"].as_str().map(str::to_owned))
            .collect();
        layers.retain(|layer| layer["source"] != name.as_str());
        let kind = source["type"].as_str().unwrap_or("unknown");
        let tiles = source["tiles"].get(0).or(source.get("url")).and_then(Value::as_str).unwrap_or("no URL");
        let ids = ids.iter().map(|id| format!("`{id}`")).collect::<Vec<_>>().join(", ");
        let reason = if kind == "raster" {
            ", and the renderer decides when to rebuild a raster tile's drawables by wall-clock timestamps, \
             so a frame with raster layers could do different work from run to run"
        } else {
            ""
        };
        notes.push(format!(
            "Removed the {kind} source `{name}` ({tiles}) and the layers that draw it ({ids}): the snapshot \
             holds vector tiles only{reason}."
        ));
    }
    Ok(notes)
}

/// The font stacks of the style's symbol layers, each as a map requests its
/// glyphs: the fonts' names joined by commas.
pub(crate) fn font_stacks(style: &Value) -> Result<BTreeSet<String>, String> {
    let mut stacks = BTreeSet::new();
    for layer in style["layers"].as_array().ok_or("the style has no layers")? {
        let Some(font) = layer["layout"].get("text-font") else {
            continue;
        };
        let names: Option<Vec<&str>> = font.as_array().and_then(|names| names.iter().map(Value::as_str).collect());
        let names = names.ok_or_else(|| format!("layer {}'s text-font isn't a list of names", layer["id"]))?;
        stacks.insert(names.join(","));
    }
    Ok(stacks)
}

/// The feature properties that the style's labels read (`["get", name]` in a
/// `text-field`).
pub(crate) fn label_properties(style: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, properties: &mut BTreeSet<String>) {
        if let Value::Array(items) = value {
            if let [Value::String(operator), Value::String(name), ..] = items.as_slice()
                && operator == "get"
            {
                properties.insert(name.clone());
            }
            for item in items {
                walk(item, properties);
            }
        }
    }
    let mut properties = BTreeSet::new();
    for layer in style["layers"].as_array().into_iter().flatten() {
        if let Some(field) = layer["layout"].get("text-field") {
            walk(field, &mut properties);
        }
    }
    properties
}

/// The source layers the style draws from its vector source.
pub(crate) fn source_layers(style: &Value) -> BTreeSet<String> {
    style["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|layer| layer["source"] == VECTOR_SOURCE)
        .filter_map(|layer| layer["source-layer"].as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localizing_keeps_the_vector_layers_and_drops_the_others() {
        let mut style = json!({
            "version": 8,
            "sources": {
                "relief": { "type": "raster", "tiles": ["https://example.com/{z}/{x}/{y}.png"] },
                "openmaptiles": { "type": "vector", "url": "https://example.com/planet" },
            },
            "sprite": "https://example.com/sprite",
            "glyphs": "https://example.com/{fontstack}/{range}.pbf",
            "layers": [
                { "id": "background", "type": "background" },
                { "id": "relief", "type": "raster", "source": "relief" },
                {
                    "id": "label",
                    "type": "symbol",
                    "source": "openmaptiles",
                    "source-layer": "place",
                    "layout": { "text-font": ["Sans", "Fallback"], "text-field": ["coalesce", ["get", "name:en"], ["get", "name"]] },
                },
            ],
        });
        let notes = localize(&mut style, "sprites/s").unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("`relief`"));
        assert_eq!(style["layers"].as_array().unwrap().len(), 2);
        assert_eq!(style["sources"]["openmaptiles"]["url"], "pmtiles://asset://tiles/street.pmtiles");
        assert_eq!(style["sprite"], "asset://sprites/s");
        assert_eq!(font_stacks(&style).unwrap(), BTreeSet::from(["Sans,Fallback".to_owned()]));
        assert_eq!(label_properties(&style), BTreeSet::from(["name".to_owned(), "name:en".to_owned()]));
        assert_eq!(source_layers(&style), BTreeSet::from(["place".to_owned()]));
    }
}
